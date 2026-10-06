"use strict";

// ssh connector: runs the app's ssh with the profile's arguments and the
// remote `sushiai proxy` (which auto-starts the daemon there), as a byte pipe
// into client.cjs. Failures are classified from ssh's exit code and stderr:
//
//   255 + "Permission denied"                          need_auth     (no retry)
//   "REMOTE HOST IDENTIFICATION HAS CHANGED" /
//     "Host key verification failed"                   failed host_key_changed (no retry)
//   127 or "No such file"                              failed not_installed    (no retry)
//   2 (the proxy: the daemon closed first)             reconnect at once
//   anything else                                      failed, backoff

const { quote } = require("../connections.cjs");
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
        "The host key changed. Check the host, then remove its old key from the sushiAI known_hosts file.",
    };
  if (code === 255 && /Permission denied/i.test(text))
    return {
      state: "need_auth",
      retry: false,
      message: `SSH needs authentication: ${detail}`,
    };
  if (code === 127 || (code !== 255 && /No such file/i.test(text)))
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

function createSshConnector({
  profile,
  ssh = "/usr/bin/ssh",
  args = () => [],
  spawnProcess = spawnPipe,
  helloTimeoutMs = HELLO_TIMEOUT_MS,
}) {
  const argv = [...args(profile), profile.host, PROXY_COMMAND];
  const describe = (exit) => {
    const { state, ...rest } = classifyExit(exit);
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
        classify(exit) {
          const { message, ...rest } = describe(exit);
          return failure(message, rest);
        },
      });
    },
    /** Why a connected client was lost: null while its exit is unknown. */
    describeLoss(client) {
      const exit = client.exit?.();
      if (!exit) return null;
      const lost = describe(exit);
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

module.exports = { createSshConnector, classifyExit, PROXY_COMMAND };
