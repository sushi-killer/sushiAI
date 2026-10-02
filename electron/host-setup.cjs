const { execFile } = require("node:child_process");
const os = require("node:os");
const path = require("node:path");
const { quote } = require("./connections.cjs");
const { checkHerdrCompatibility } = require("./herdr-compatibility.cjs");
const {
  installPinnedHerdr,
  installRemoteHerdr,
} = require("./herdr-install.cjs");
const { errorDetails } = require("./herdr.cjs");

const HOST_PATH = `export PATH="$HOME/.local/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"
# CLIs installed with npm (nvm, a user prefix) count as installed.
for d in "$HOME"/.nvm/versions/node/*/bin "$HOME/.npm-global/bin"; do
  [ -d "$d" ] && PATH="$PATH:$d"
done
`;

const SETUP_SCRIPT = `set -u
${HOST_PATH}
have() { command -v "$1" >/dev/null 2>&1; }
say() { printf 'SUSHIAI_SETUP %s %s\\n' "$1" "$2"; }
# The app reads a host's files with python3; it is not installed here.
have python3 || say python3 missing
have curl || { say curl missing; exit 0; }
if have claude; then say claude present
elif curl -fsSL https://claude.ai/install.sh | bash >&2 && have claude; then say claude installed
else say claude failed; fi
if have codex; then say codex present
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) t=x86_64-unknown-linux-musl ;;
    Linux-aarch64) t=aarch64-unknown-linux-musl ;;
    Darwin-arm64) t=aarch64-apple-darwin ;;
    Darwin-x86_64) t=x86_64-apple-darwin ;;
    *) t= ;;
  esac
  d=$(mktemp -d)
  if [ -n "$t" ] && curl -fsSL "https://github.com/openai/codex/releases/latest/download/codex-$t.tar.gz" | tar -xz -C "$d" >&2 \\
    && mkdir -p "$HOME/.local/bin" && mv "$d/codex-$t" "$HOME/.local/bin/codex" && chmod +x "$HOME/.local/bin/codex"; then
    say codex installed
  else say codex failed; fi
  rm -rf "$d"
fi
`;

/** `{ herdr: "installed", claude: "present", ... }` from the script's output. */
function parseSetup(output) {
  const states = {};
  for (const [, tool, state] of String(output).matchAll(
    /^SUSHIAI_SETUP (\S+) (\S+)$/gm,
  ))
    states[tool] = state;
  return states;
}

/** One line for the owner: what was installed or started, and what failed. */
function setupSummary(states) {
  const done = Object.entries(states)
    .filter(([, state]) => ["installed", "started"].includes(state))
    .map(([tool, state]) => `${tool} ${state}`);
  const failed = Object.entries(states)
    .filter(([, state]) =>
      ["failed", "missing", "incompatible"].includes(state),
    )
    .map(([tool, state]) => `${tool} ${state}`);
  return [
    ...done,
    ...failed,
    ...(states.herdrError ? [states.herdrError.message] : []),
    ...(states.compatibility && !states.compatibility.compatible
      ? states.compatibility.issues
      : []),
  ].join(" · ");
}

/** What a Herdr server started here may see: it outlives the app and hands
 * its environment to every pane, so nothing of the app's own (Electron, npm,
 * a Claude Code session the app was started from) goes in. */
function cleanEnvironment(env = process.env) {
  const keep = ["HOME", "PATH", "USER", "LOGNAME", "SHELL", "LANG", "TMPDIR"];
  return Object.fromEntries(
    keep.filter((name) => env[name]).map((name) => [name, env[name]]),
  );
}

/** Points Herdr at the socket selected by the connection. */
function socketLine(socket) {
  if (!socket) return "";
  const where = socket.startsWith("~/")
    ? `"$HOME"/${quote(socket.slice(2))}`
    : quote(socket);
  return `export HERDR_SOCKET_PATH=${where}\n`;
}

/** Setups under way, by endpoint and socket: a second connect waits for the first
 * instead of running the installers twice. */
const running = new Map();

/** Runs the setup on an SSH host (over its connection, for its Herdr
 * `socket`) or on the local machine. */
function setupHost(connections, endpoint, socket = "", options = {}) {
  const remote = typeof endpoint === "string" && endpoint.startsWith("ssh:");
  const selected = remote
    ? socket || connections.get(endpoint).socket
    : socket ||
      (typeof endpoint === "string" && path.isAbsolute(endpoint)
        ? endpoint
        : process.env.HERDR_SOCKET_PATH ||
          path.join(os.homedir(), ".config/herdr/herdr.sock"));
  if (remote && selected !== connections.get(endpoint).socket)
    return Promise.reject(
      new Error("Setup socket must match the SSH connection's socket."),
    );
  if (!remote && !path.isAbsolute(selected))
    return Promise.reject(
      new Error("Choose an absolute local Herdr socket path."),
    );
  const key = JSON.stringify([remote ? endpoint : "local", selected]);
  if (!running.has(key)) {
    const run = Promise.resolve()
      .then(() =>
        runSetup(connections, remote ? endpoint : selected, selected, options),
      )
      .finally(() => running.delete(key));
    running.set(key, run);
  }
  return running.get(key);
}

function localScript(script, timeout) {
  return new Promise((resolve, reject) => {
    const child = execFile(
      "/bin/sh",
      ["-s"],
      { timeout, maxBuffer: 4 * 1024 * 1024, env: cleanEnvironment() },
      (error, stdout) => (error ? reject(error) : resolve(stdout)),
    );
    child.stdin.end(script);
  });
}

function startServerScript(binary, socket) {
  const command = quote(binary);
  return `${HOST_PATH}${socketLine(socket)}
say() { printf 'SUSHIAI_SETUP server %s\\n' "$1"; }
if ${command} status server 2>/dev/null | grep -q '^status: running'; then say running
elif [ -S "\${HERDR_SOCKET_PATH:-$HOME/.config/herdr/herdr.sock}" ]; then say failed
else
  nohup ${command} server >/dev/null 2>&1 </dev/null &
  for i in 1 2 3 4 5 6 7 8 9 10; do
    ${command} status server 2>/dev/null | grep -q '^status: running' && break
    sleep 1
  done
  if ${command} status server 2>/dev/null | grep -q '^status: running'; then say started
  else say failed; fi
fi
`;
}

async function runSetup(
  connections,
  endpoint,
  socket,
  {
    binary,
    installLocal = installPinnedHerdr,
    installRemote = installRemoteHerdr,
    checkCompatibility = checkHerdrCompatibility,
    runLocalScript = localScript,
  } = {},
) {
  const timeout = 10 * 60 * 1000;
  const remote = endpoint.startsWith("ssh:");
  const runScript = (script) =>
    remote
      ? connections.exec(endpoint, "sh -s", { input: script, timeout })
      : runLocalScript(script, timeout);
  const states = parseSetup(await runScript(SETUP_SCRIPT));
  const probe = () =>
    checkCompatibility({
      endpoint,
      connections,
      binary,
      preferManaged: false,
    });
  if (!binary)
    binary = (await runScript(`${HOST_PATH}command -v herdr || true\n`)).trim();
  states.herdrInstallation = binary ? "user" : "managed";
  if (!binary) {
    try {
      const installed = remote
        ? await installRemote(endpoint, connections)
        : await installLocal(connections.herdrInstallDirectory);
      binary = installed.binary;
      states.herdr = installed.installed ? "installed" : "present";
    } catch (error) {
      states.herdr = "failed";
      states.herdrError = errorDetails(error);
      states.compatibility = await probe();
      states.server = states.compatibility.daemon.available
        ? states.compatibility.daemon.compatible
          ? "running"
          : "incompatible"
        : "failed";
      return states;
    }
  } else states.herdr = "present";
  let status = await probe();
  // The daemon may still run a release whose CLI is another one on disk;
  // sessions attach through it (see checkHerdrCompatibility), so setup does too.
  if (!status.compatible && status.daemon.compatible) {
    const other = await checkCompatibility({ endpoint, connections, binary });
    if (other.compatible) status = other;
  }
  if (!status.cli.compatible) {
    states.herdr = "incompatible";
    states.server = status.daemon.available
      ? status.daemon.compatible
        ? "running"
        : "incompatible"
      : "failed";
  } else if (status.daemon.available) {
    states.server = status.daemon.compatible ? "running" : "incompatible";
  } else {
    states.server = parseSetup(
      await runScript(startServerScript(binary, socket)),
    ).server;
    status = await probe();
    if (!status.daemon.compatible)
      states.server = status.daemon.available ? "incompatible" : "failed";
  }
  states.compatibility = status;
  return states;
}

module.exports = {
  SETUP_SCRIPT,
  parseSetup,
  setupSummary,
  setupHost,
  cleanEnvironment,
};
