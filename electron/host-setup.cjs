const fs = require("node:fs");
const { execFile } = require("node:child_process");
const path = require("node:path");
const {
  installSushiai,
  parseProbe,
  REMOTE_PATH,
} = require("./host-install.cjs");
const { quote } = require("./connections.cjs");
const { syncBuiltinSkillsOnHost } = require("./extensions/builtin-skills.cjs");

const SETUP_SCRIPT = `set -u
${REMOTE_PATH}
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

// `claude auth status` / `codex login status` exit non-zero when logged out.
// Bounded with `timeout` when the host has it: a CLI waiting on a prompt must
// not hang the probe.
const PREFLIGHT_SCRIPT = `${REMOTE_PATH}
t() { if command -v timeout >/dev/null 2>&1; then timeout 10 "$@"; else "$@"; fi; }
command -v git >/dev/null 2>&1 && echo "git=1"
if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1; then echo "cc=1"; fi
if command -v claude >/dev/null 2>&1; then echo "claude=1"; t claude auth status >/dev/null 2>&1 && echo "claude_login=1"; fi
if command -v codex >/dev/null 2>&1; then echo "codex=1"; t codex login status >/dev/null 2>&1 && echo "codex_login=1"; fi
exit 0`;

/** What a host offers the orchestrator's routes: `git` plus each harness
 * CLI's presence and login. */
function parsePreflight(output, now = Date.now()) {
  const values = parseProbe(output);
  const harness = (name) => ({
    installed: values[name] === "1",
    loggedIn: values[`${name}_login`] === "1",
  });
  return {
    git: values.git === "1",
    cc: values.cc === "1",
    claude: harness("claude"),
    codex: harness("codex"),
    checkedAt: now,
  };
}

/** `{ manifest, binDir }` from the manifest file `npm run build:host` writes
 * into target/host, or null when it is not there. The caller passes the result
 * as the `sushiai` option of setupHost. */
function loadHostManifest(file) {
  try {
    const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
    return { manifest, binDir: path.dirname(file) };
  } catch {
    return null;
  }
}

/** Where the host binaries and their manifest live: the app's resources when
 * packaged, else the repo's target/host (`npm run build:host`). */
function hostManifestFile({
  isPackaged = false,
  resourcesPath = process.resourcesPath,
  repoRoot = path.join(__dirname, ".."),
} = {}) {
  return isPackaged && resourcesPath
    ? path.join(resourcesPath, "host", "manifest.json")
    : path.join(repoRoot, "target", "host", "manifest.json");
}

/** `loadHostManifest` that throws a message the owner can act on. */
function requireHostManifest(options = {}) {
  const file = hostManifestFile(options);
  const loaded = loadHostManifest(file);
  if (loaded) return loaded;
  throw new Error(
    options.isPackaged
      ? "This build of sushiAI has no sushiai binaries for remote hosts."
      : `No host manifest at ${file}. Run npm run build:host first.`,
  );
}

/** `{ claude: "present", codex: "installed", ... }` from the script's output. */
function parseSetup(output) {
  const states = {};
  for (const [, tool, state] of String(output).matchAll(
    /^SUSHIAI_SETUP (\S+) (\S+)$/gm,
  ))
    states[tool] = state;
  return states;
}

/** One line for the owner: what was installed, and what failed. */
function setupSummary(states) {
  const done = Object.entries(states)
    .filter(([, state]) => state === "installed")
    .map(([tool, state]) => `${tool} ${state}`);
  const failed = Object.entries(states)
    .filter(([, state]) => ["failed", "missing"].includes(state))
    .map(([tool, state]) => `${tool} ${state}`);
  return [
    ...done,
    ...failed,
    ...(states.sushiaiError ? [states.sushiaiError] : []),
  ].join(" · ");
}

/** What a setup script run here may see: nothing of the app's own (Electron,
 * npm, a Claude Code session the app was started from) goes in. */
function cleanEnvironment(env = process.env) {
  const keep = ["HOME", "PATH", "USER", "LOGNAME", "SHELL", "LANG", "TMPDIR"];
  return Object.fromEntries(
    keep.filter((name) => env[name]).map((name) => [name, env[name]]),
  );
}

/** Runs the setup on an SSH host (over its connection) or on the local
 * machine. */
function setupHost(connections, endpoint, options = {}) {
  return runSetup(connections, endpoint, options);
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

async function runSetup(
  connections,
  endpoint,
  {
    runLocalScript = localScript,
    installSkill = syncBuiltinSkillsOnHost,
    sushiai,
    installSushiaiBinary = installSushiai,
  } = {},
) {
  const timeout = 10 * 60 * 1000;
  const remote = typeof endpoint === "string" && endpoint.startsWith("ssh:");
  if (remote && connections.hasShell && !connections.hasShell(endpoint))
    return {
      note: "This host connects through a command and has no shell: set up sushiai, Claude Code and Codex on it yourself.",
    };
  if (remote) void installSkill(connections, endpoint);
  const runScript = (script) =>
    remote
      ? connections.exec(endpoint, "sh -s", { input: script, timeout })
      : runLocalScript(script, timeout);
  const states = parseSetup(await runScript(SETUP_SCRIPT));
  // `sushiai` is `{ manifest, binDir }` (loadHostManifest). Remote hosts only:
  // the local machine runs the bundled daemon itself.
  if (remote && sushiai) {
    try {
      const result = await installSushiaiBinary({
        exec: (command, o) => connections.exec(endpoint, command, o),
        manifest: sushiai.manifest,
        binDir: sushiai.binDir,
      });
      states.sushiai = result.status;
      states.sushiaiVersion = result.version;
      states.sushiaiResult = result;
    } catch (error) {
      states.sushiai = "failed";
      states.sushiaiError = error.message;
    }
  }
  return states;
}

// One setup per host at a time: the first-ready tool setup and a manual
// install share the host's in-flight promise, so a second call starts only
// when the first has settled (it keeps its own options).
function serializePerHost(setup) {
  const inFlight = new Map();
  return (endpoint, options) => {
    const previous = inFlight.get(endpoint) || Promise.resolve();
    const run = previous.catch(() => {}).then(() => setup(endpoint, options));
    const tail = run.catch(() => {});
    inFlight.set(endpoint, tail);
    void tail.then(() => {
      if (inFlight.get(endpoint) === tail) inFlight.delete(endpoint);
    });
    return run;
  };
}

// Stops the daemon of an older binary that cannot take `daemon.shutdown`. The
// pid in daemon.lock is trusted only when the lock is held: taking the flock
// fails then. After a crash the file keeps a stale pid, which must never be
// signalled.
const STOP_DAEMON_PY = `
import fcntl, os, signal, sys
path = os.path.join(os.path.expanduser("~"), ".sushiai", "daemon.lock")
try:
    handle = open(path, "r+")
except OSError:
    print("nolock")
    sys.exit(0)
try:
    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    print("free")
    sys.exit(0)
except OSError:
    pass
try:
    pid = int(handle.read().strip())
except ValueError:
    print("nopid")
    sys.exit(0)
if pid > 1:
    os.kill(pid, signal.SIGTERM)
    print("stopped")
`;
const STOP_DAEMON_COMMAND = `sh -c ${quote(
  `${REMOTE_PATH}; python3 -c ${quote(STOP_DAEMON_PY)}`,
)}`;

const READY_ATTEMPTS = 6;
const SETTLE_MS = 400;
// Another attempt cannot help: the owner has to act.
const needsOwner = (state) =>
  state.state === "need_auth" || state.reason === "host_key_changed";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** manager.retry(host) until the host is ready or its state needs the owner;
 * resolves the last state (`before` when no attempt ran). */
async function reconnectUntilReady(
  manager,
  host,
  { attempts = READY_ATTEMPTS, settleMs = SETTLE_MS, before } = {},
) {
  let state = before;
  for (let i = 0; i < attempts; i++) {
    state = await manager.retry(host);
    if (state.state === "ready") return state;
    if (needsOwner(state)) break;
    await sleep(settleMs);
  }
  return state;
}

/** The `host-install` handler. A remote host: setupHost with the host manifest
 * (tools, skills, then the bundled sushiai), the old daemon stops
 * (`daemon.shutdown` when connected; sessions live in their holders and
 * survive), then manager.retry(host) until ready or a state that needs the
 * owner. "local": the "Restart daemon" action, which stops whatever daemon
 * answers on this Mac and starts this app's build. */
function createHostInstaller({
  manager,
  connections,
  manifest,
  setup,
  restartLocal = async () => {},
  markSetup = () => {},
  settleMs = SETTLE_MS,
  attempts = READY_ATTEMPTS,
}) {
  const untilReady = (host, before) =>
    reconnectUntilReady(manager, host, { attempts, settleMs, before });
  async function installOn(host) {
    if (host === "local") {
      await restartLocal();
      const state = await untilReady(host);
      if (state?.state !== "ready")
        throw new Error(
          `The daemon did not restart: ${state?.message || state?.state || "unknown"}`,
        );
      return { status: "restarted", version: state.version || "" };
    }
    const endpoint = `ssh:${host}`;
    // Throws "unknown host" before anything runs.
    if (!connections.hasShell(endpoint))
      throw new Error(
        "This host connects through a command. Install sushiai there yourself.",
      );
    const { manifest: entries, binDir } = manifest();
    const states = await setup(endpoint, {
      sushiai: { manifest: entries, binDir },
    });
    if (states.sushiai === "failed")
      throw new Error(states.sushiaiError || "Installing sushiai failed.");
    const result = states.sushiaiResult;
    if (!result) throw new Error("The installer did not report sushiai.");
    markSetup(host);
    const before = manager.states().find((state) => state.host === host);
    if (before?.state === "ready") {
      await manager.request(host, "daemon.shutdown", {}).catch(() => {});
    } else {
      await connections
        .exec(endpoint, STOP_DAEMON_COMMAND, { timeout: 30000 })
        .catch(() => {});
    }
    await sleep(settleMs);
    const state = await untilReady(host, before);
    if (state?.state === "ready") return result;
    throw new Error(
      `sushiai was installed, but the daemon on the host is not ready: ${state?.message || state?.state || "unknown"}`,
    );
  }
  return { install: installOn };
}

module.exports = {
  SETUP_SCRIPT,
  createHostInstaller,
  serializePerHost,
  reconnectUntilReady,
  STOP_DAEMON_COMMAND,
  PREFLIGHT_SCRIPT,
  parsePreflight,
  parseSetup,
  setupSummary,
  setupHost,
  cleanEnvironment,
  loadHostManifest,
  hostManifestFile,
  requireHostManifest,
};
