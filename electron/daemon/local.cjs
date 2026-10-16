"use strict";

// Local connector: finds (or starts) the bundled `sushiai` daemon for this
// user and connects to it. The daemon outlives the app; only clients start it
// (ARCHITECTURE.md "Daemon control"), so this file is its single local starter.
//
//   home    $SUSHIAI_HOME, else ~/.sushiai, created 0700
//   binary  $SUSHIAI_DAEMON_BIN, else <resources>/sushiai when packaged,
//           else the newer (mtime) of the repo's target/release and target/debug
//   build   `hello.build` is the sha256 of the running daemon's own binary.
//           The crate version is no compatibility test (it never changes);
//           `hello.capabilities` is: a daemon of another build is shared only
//           when it serves every REQUIRED_CAPABILITIES entry. Otherwise a dev
//           run replaces it once per app run; a packaged app never replaces a
//           live daemon: it connects and marks it `needsUpdate` ("restart the
//           daemon to update"). After the one replacement a mismatch is
//           "incompatible" and retried slowly. <home>/bin/sushiai belongs to
//           the daemon itself.
//   hooks   `sushiai hooks install` once per app version (stamp file in home);
//           never in a test run (it would write the owner's ~/.codex) and
//           never for a home other than ~/.sushiai (the hooks would point to it)

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const childProcess = require("node:child_process");
const { connectDaemon } = require("./client.cjs");

const START_TIMEOUT_MS = 15000;
const STOP_TIMEOUT_MS = 8000;
const POLL_MS = 100;
const HOOKS_STAMP = "hooks-installed";
// The daemon capabilities this app uses (crates/sushiai-protocol CAPABILITIES).
const REQUIRED_CAPABILITIES = [
  "sessions",
  "attach",
  "resync",
  "agents",
  "catalog",
  "catalogRead",
  "hibernate",
  "schedules",
  "ownerAsk",
];
// A failed "incompatible" connect is looked at again on this steady beat.
const INCOMPATIBLE_RETRY_MS = 30000;
// What a detached daemon (and every session it starts) may see of the app's
// own environment: nothing of Electron, npm or the shell the app came from.
const DAEMON_ENV_KEYS = [
  "HOME",
  "PATH",
  "USER",
  "LOGNAME",
  "SHELL",
  "LANG",
  "LC_ALL",
  "LC_CTYPE",
  "TMPDIR",
  "SSH_AUTH_SOCK",
  "CODEX_HOME",
  "SUSHIAI_HOME",
  "SUSHIAI_HOST",
  "SUSHIAI_LOG",
];

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function resolveHome(env = process.env) {
  return env.SUSHIAI_HOME || path.join(os.homedir(), ".sushiai");
}

/** Creates the home with mode 0700; the daemon refuses anything looser. */
function ensureHome(home) {
  let stat = null;
  try {
    stat = fs.lstatSync(home);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  if (!stat) fs.mkdirSync(home, { recursive: true, mode: 0o700 });
  else if (!stat.isDirectory() || stat.isSymbolicLink())
    throw new Error(
      `${home} must be a real directory with mode 0700, not a symlink or a file.`,
    );
  fs.chmodSync(home, 0o700);
  return home;
}

function resolveBinary({
  env = process.env,
  isPackaged = false,
  resourcesPath = process.resourcesPath,
  repoRoot = path.join(__dirname, "..", ".."),
  exists = fs.existsSync,
  mtimeOf = (file) => fs.statSync(file).mtimeMs,
} = {}) {
  if (env.SUSHIAI_DAEMON_BIN) return env.SUSHIAI_DAEMON_BIN;
  if (isPackaged && resourcesPath) {
    const packaged = path.join(resourcesPath, "sushiai");
    if (!exists(packaged))
      throw new Error(
        `This build of sushiAI is missing its bundled daemon (${packaged}). Reinstall the app, or rebuild the package with \`npm run build:daemon && npm run build:host\` first.`,
      );
    return packaged;
  }
  let best = null;
  for (const profile of ["release", "debug"]) {
    const candidate = path.join(repoRoot, "target", profile, "sushiai");
    if (!exists(candidate)) continue;
    const mtime = mtimeOf(candidate);
    if (!best || mtime > best.mtime) best = { candidate, mtime };
  }
  if (best) return best.candidate;
  throw new Error(
    "sushiai binary not found: run `npm run build:daemon` or set SUSHIAI_DAEMON_BIN",
  );
}

function sha256File(file) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    fs.createReadStream(file)
      .on("data", (chunk) => hash.update(chunk))
      .on("error", reject)
      .on("end", () => resolve(hash.digest("hex")));
  });
}

function daemonEnv(env, home) {
  const out = {};
  for (const key of DAEMON_ENV_KEYS) if (env[key]) out[key] = env[key];
  out.SUSHIAI_HOME = home;
  // Sessions inherit this: without a locale a terminal garbles non-ASCII text.
  if (!out.LANG && !out.LC_ALL) out.LANG = "en_US.UTF-8";
  return out;
}

function createLocalConnector({
  env = process.env,
  appVersion,
  isPackaged,
  resourcesPath,
  repoRoot,
  spawn = childProcess.spawn,
  execFile = childProcess.execFile,
  connect = connectDaemon,
  startTimeoutMs = START_TIMEOUT_MS,
  stopTimeoutMs = STOP_TIMEOUT_MS,
  pollMs = POLL_MS,
  log = () => {},
  testMode = Boolean(env.SUSHIAI_TEST_WINDOW),
  defaultHome = path.join(os.homedir(), ".sushiai"),
} = {}) {
  const home = resolveHome(env);
  const socketPath = path.join(home, "daemon.sock");
  let binary = "";
  let bundledVersion = "";
  let binarySha = "";
  let shaOf = "";
  let replaced = false; // this app run replaced a daemon once already

  function run(args) {
    return new Promise((resolve, reject) => {
      execFile(
        binary,
        args,
        { env: daemonEnv(env, home), timeout: 30000 },
        (error, stdout, stderr) =>
          error
            ? reject(
                new Error(
                  `sushiai ${args.join(" ")}: ${stderr || error.message}`,
                ),
              )
            : resolve(String(stdout)),
      );
    });
  }

  async function installHooksOnce() {
    const stamp = path.join(home, HOOKS_STAMP);
    try {
      if (fs.readFileSync(stamp, "utf8").trim() === appVersion) return false;
    } catch {
      // No stamp, or an unreadable one: install again.
    }
    await run(["hooks", "install"]);
    fs.writeFileSync(stamp, `${appVersion}\n`, { mode: 0o600 });
    return true;
  }

  async function prepare() {
    ensureHome(home);
    binary = resolveBinary({ env, isPackaged, resourcesPath, repoRoot });
    if (!bundledVersion)
      bundledVersion = (await run(["--version"])).trim().split(/\s+/).pop();
    const stat = fs.statSync(binary);
    const key = `${binary}:${stat.size}:${stat.mtimeMs}`;
    if (shaOf !== key) {
      binarySha = await sha256File(binary);
      shaOf = key;
    }
    // A test run must never write the owner's Codex files or bin link.
    if (testMode) return;
    // Hooks name the home they were installed from: a throwaway home would
    // leave ~/.codex pointing at it.
    if (home !== defaultHome) return;
    try {
      await installHooksOnce();
    } catch (error) {
      // Hooks are not needed to run sessions; retried on the next connect.
      log(`hooks install failed: ${error.message}`);
    }
  }

  // Does the running daemon come from the binary resolved now?
  const isCurrent = (client) =>
    client.hello.daemon === bundledVersion && client.hello.build === binarySha;

  // Another build works for this app when it serves everything the app uses.
  const shareable = (client) =>
    REQUIRED_CAPABILITIES.every((name) =>
      client.hello.capabilities?.includes(name),
    );

  /** A daemon of another build that stays: the manager learns it is foreign,
   * and `needsUpdate` when it lacks what this app uses. */
  function shared(client, needsUpdate) {
    client.foreignBuild = true;
    if (needsUpdate) client.needsUpdate = true;
    return client;
  }

  function incompatible(client) {
    client.close();
    return Object.assign(
      new Error(
        `Another sushiAI build owns the daemon on this Mac (${client.hello.daemon}, bundled ${bundledVersion}). Restart the daemon to use this app's build.`,
      ),
      { reason: "incompatible", retryAfterMs: INCOMPATIBLE_RETRY_MS },
    );
  }

  // `exited` resolves when the started child ends (a daemon that loses the
  // lock to another one exits after a few seconds); `failed` rejects when it
  // cannot start at all.
  function startDaemon() {
    const fd = fs.openSync(path.join(home, "daemon.log"), "a", 0o600);
    try {
      const child = spawn(binary, ["daemon"], {
        detached: true,
        stdio: ["ignore", fd, fd],
        env: daemonEnv(env, home),
      });
      const failed = new Promise((_, reject) =>
        child.once("error", (error) =>
          reject(new Error(`cannot start sushiai daemon: ${error.message}`)),
        ),
      );
      failed.catch(() => {});
      const exited = new Promise((resolve) =>
        child.once("exit", (code, signal) => resolve({ code, signal })),
      );
      child.unref?.();
      return { failed, exited, pid: child.pid };
    } finally {
      fs.closeSync(fd);
    }
  }

  let lastConnectError = "";
  async function tryConnect() {
    try {
      return await connect({ socketPath, clientName: "desktop" });
    } catch (error) {
      // Nobody listening is the normal answer while a daemon starts; log each
      // distinct reason once.
      if (error?.message !== lastConnectError) {
        lastConnectError = error?.message;
        log(`daemon connect failed: ${lastConnectError}`);
      }
      return null;
    }
  }

  async function connectWithin(ms, starting) {
    const deadline = Date.now() + ms;
    let exit = null;
    starting.exited.then((info) => (exit = info));
    for (;;) {
      const client = await tryConnect();
      if (client) return client;
      if (exit)
        throw new Error(
          `sushiai daemon exited before it answered (${exit.signal || `code ${exit.code}`}); see ${path.join(home, "daemon.log")}`,
        );
      if (Date.now() >= deadline)
        throw new Error(
          `sushiai daemon did not come up within ${Math.round(ms / 1000)} s (see ${path.join(home, "daemon.log")})`,
        );
      await Promise.race([sleep(pollMs), starting.failed, starting.exited]);
    }
  }

  async function waitGone(client) {
    client.close();
    const deadline = Date.now() + stopTimeoutMs;
    while (Date.now() < deadline) {
      const probe = await tryConnect();
      if (!probe) return;
      probe.close();
      await sleep(pollMs);
    }
    throw new Error("old sushiai daemon did not stop");
  }

  return {
    kind: "local",
    home,
    socketPath,
    // The owner's "Restart daemon": stops whatever daemon answers (sessions
    // survive in their holders) so the next connect starts this app's build.
    async restart() {
      const client = await tryConnect();
      if (client) {
        await client.request("daemon.shutdown", {}).catch(() => {});
        await waitGone(client);
      }
      replaced = false;
    },
    /** Runs the resolved binary with the daemon's environment and resolves its
     * stdout: a module's own CLI (the desktop's enable flow). */
    async runCli(args) {
      ensureHome(home);
      binary ||= resolveBinary({ env, isPackaged, resourcesPath, repoRoot });
      return run(args);
    },
    async connect() {
      await prepare();
      let client = await tryConnect();
      let fresh = false; // started by this call
      for (;;) {
        if (client && isCurrent(client)) return client;
        if (client) {
          // Another build that serves everything this app uses is shared (its
          // sessions stay), whoever started it.
          if (shareable(client)) return shared(client, false);
          // A packaged app never stops a live daemon: it works with it and
          // tells the owner to restart the daemon to update.
          if (isPackaged) return shared(client, true);
          // A dev run replaces it once; a daemon that shows up after that
          // (another app won the race) is not this build's to replace.
          if (replaced || fresh) throw incompatible(client);
          replaced = true;
          log(
            `daemon ${client.hello.daemon} is not the bundled binary; replacing`,
          );
          // Sessions live in their holders and survive the restart (D5).
          await client.request("daemon.shutdown", {}).catch(() => {});
          await waitGone(client);
        }
        client = await connectWithin(startTimeoutMs, startDaemon());
        fresh = true;
      }
    },
  };
}

module.exports = {
  createLocalConnector,
  resolveHome,
  daemonEnv,
  ensureHome,
  resolveBinary,
  HOOKS_STAMP,
  REQUIRED_CAPABILITIES,
};
