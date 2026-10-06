"use strict";

// ssh connector: runs the app's ssh with the profile's arguments and the
// remote `sushiai proxy` (which auto-starts the daemon there), as a byte pipe
// into client.cjs. Failures are classified from ssh's exit code and stderr:
//
//   255 + "Permission denied"                          need_auth     (no retry; a key
//                                                       or the ssh agent is needed)
//   "REMOTE HOST IDENTIFICATION HAS CHANGED" /
//     "Host key verification failed"                   failed host_key_changed (no retry)
//   127 or "No such file" for the sushiai path         failed not_installed    (no retry)
//   a daemon speaking another protocol                 failed incompatible     (no retry)
//   2 (the proxy: the daemon closed first)             reconnect at once
//   anything else                                      failed, backoff

const { quote, run } = require("../connections.cjs");
const {
  spawnPipe,
  connectOverPipe,
  failure,
  tail,
} = require("./connectors.cjs");

const HELLO_TIMEOUT_MS = 20000;
const DAEMON_DIED = 2;
// sshd hands this to the login shell; `sh -c` keeps fish/csh out of it and
// $HOME expands to the absolute home on the host.
const PROXY_COMMAND = `sh -c ${quote('"$HOME/.sushiai/bin/sushiai" proxy')}`;

/** What an ssh/proxy exit means: { state, reason?, message, retry?, retryNow? }. */
function classifyExit({ code, signal, stderr = "", error } = {}) {
  const text = String(stderr);
  const detail = tail(text) || error?.message || "";
  if (
    /REMOTE HOST IDENTIFICATION HAS CHANGED|Host key verification failed/i.test(
      text,
    )
  )
    return {
      state: "failed",
      reason: "host_key_changed",
      retry: false,
      message:
        "The host key changed. If you trust the change, remove the old key with the command below and connect again.",
    };
  if (code === 255 && /Permission denied/i.test(text))
    return {
      state: "need_auth",
      retry: false,
      message: `SSH needs a key or the ssh agent: sushiAI connects in batch mode, without a prompt. ${detail}`,
    };
  if (
    code === 127 ||
    (code !== 255 && /\.sushiai\/bin\/sushiai[^\n]*No such file/i.test(text))
  )
    return {
      state: "failed",
      reason: "not_installed",
      retry: false,
      message: "sushiai is not installed on this host. Install it first.",
    };
  if (code === DAEMON_DIED)
    return {
      state: "failed",
      reason: "daemon_died",
      retryNow: true,
      message: "The daemon on the host stopped.",
    };
  return {
    state: "failed",
    message:
      detail ||
      (signal ? `ssh ended with ${signal}` : `ssh exited with ${code}`),
  };
}

/** `hostname` and `port` of `ssh -G`, as ssh resolves the alias. */
async function resolveWithSsh(ssh, args, profile) {
  const out = await run(ssh, [...args(profile), "-G", profile.host], "", 3000);
  const value = (key) =>
    new RegExp(`^${key} (\\S+)$`, "m").exec(out)?.[1] || undefined;
  return { hostname: value("hostname"), port: value("port") };
}

function createSshConnector({
  profile,
  ssh = "/usr/bin/ssh",
  args = () => [],
  knownHostsFile,
  resolveHost = () => resolveWithSsh(ssh, args, profile),
  spawnProcess = spawnPipe,
  helloTimeoutMs = HELLO_TIMEOUT_MS,
}) {
  const argv = [...args(profile), profile.host, PROXY_COMMAND];
  // The exact command that removes the changed key from the app's own
  // known_hosts file, for the name ssh stores it under.
  async function keygenHint() {
    let name = profile.host.replace(/^.*@/, "");
    let port = profile.port;
    try {
      const resolved = await resolveHost();
      name = resolved.hostname || name;
      port = port || (resolved.port !== "22" ? resolved.port : undefined);
    } catch {
      // Keep the alias: it is what ssh was given.
    }
    const entry = port && Number(port) !== 22 ? `[${name}]:${port}` : name;
    return `ssh-keygen -R ${quote(entry)}${knownHostsFile ? ` -f ${quote(knownHostsFile)}` : ""}`;
  }
  const describe = async (exit) => {
    const { state, ...rest } = classifyExit(exit);
    if (rest.reason === "host_key_changed") rest.hint = await keygenHint();
    return { state, ...rest };
  };
  return {
    kind: "ssh",
    // A change here replaces the connector (and reconnects).
    signature: JSON.stringify(["ssh", ssh, argv]),
    connect() {
      return connectOverPipe({
        pipe: spawnProcess(ssh, argv),
        helloTimeoutMs,
        async classify(exit) {
          const { message, ...rest } = await describe(exit);
          return failure(message, rest);
        },
      });
    },
    /** Why a connected client was lost: null while its exit is unknown. */
    async describeLoss(client) {
      const exit = client.exit?.();
      if (!exit) return null;
      const lost = await describe(exit);
      if (lost.reason === "daemon_died" || lost.reason === undefined)
        return {
          ...lost,
          state: "offline",
          reason: lost.reason || "connection_lost",
          message:
            lost.reason === "daemon_died"
              ? "The daemon on the host stopped."
              : `The connection to the host was lost. ${lost.message}`.trim(),
        };
      return lost;
    },
  };
}

module.exports = {
  createSshConnector,
  classifyExit,
  resolveWithSsh,
  PROXY_COMMAND,
};
