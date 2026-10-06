const fs = require("node:fs");
const { execFile } = require("node:child_process");
const path = require("node:path");
const { installSushiai } = require("./host-install.cjs");
const { syncBuiltinSkillsOnHost } = require("./extensions/builtin-skills.cjs");

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

/** Setups under way, by endpoint: a second connect waits for the first
 * instead of running the installers twice. */
const running = new Map();

/** Runs the setup on an SSH host (over its connection) or on the local
 * machine. */
function setupHost(connections, endpoint, options = {}) {
  const key = typeof endpoint === "string" ? endpoint : "local";
  if (!running.has(key)) {
    const run = Promise.resolve()
      .then(() => runSetup(connections, endpoint, options))
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

module.exports = {
  SETUP_SCRIPT,
  parseSetup,
  setupSummary,
  setupHost,
  cleanEnvironment,
  loadHostManifest,
  hostManifestFile,
  requireHostManifest,
};
