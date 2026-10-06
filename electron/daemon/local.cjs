"use strict";

// Local connector: finds (or starts) the bundled `sushiai` daemon for this
// user and connects to it. The daemon outlives the app; only clients start it
// (ARCHITECTURE.md "Daemon control"), so this file is its single local starter.
//
//   home    $SUSHIAI_HOME, else ~/.sushiai, created 0700
//   binary  $SUSHIAI_DAEMON_BIN, else <resources>/sushiai when packaged,
//           else the repo's target/release, then target/debug
//   link    <home>/bin/sushiai -> binary (what agent hooks and `sushiai open` call)
//   hooks   `sushiai hooks install` once per app version (stamp file in home)

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const childProcess = require("node:child_process");
const { connectDaemon } = require("./client.cjs");

const START_TIMEOUT_MS = 15000;
const STOP_TIMEOUT_MS = 8000;
const POLL_MS = 100;
const HOOKS_STAMP = "hooks-installed";
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
    throw new Error(`${home} must be a real directory`);
  fs.chmodSync(home, 0o700);
  return home;
}

function resolveBinary({
  env = process.env,
  isPackaged = false,
  resourcesPath = process.resourcesPath,
  repoRoot = path.join(__dirname, "..", ".."),
  exists = fs.existsSync,
} = {}) {
  if (env.SUSHIAI_DAEMON_BIN) return env.SUSHIAI_DAEMON_BIN;
  if (isPackaged && resourcesPath) return path.join(resourcesPath, "sushiai");
  for (const profile of ["release", "debug"]) {
    const candidate = path.join(repoRoot, "target", profile, "sushiai");
    if (exists(candidate)) return candidate;
  }
  throw new Error(
    "sushiai binary not found: run `npm run build:daemon` or set SUSHIAI_DAEMON_BIN",
  );
}

/** Points <home>/bin/sushiai at `binary` through a temp link and a rename. A
 * regular file (or directory) already there is the owner's and is left alone.
 * Returns "created", "updated", "unchanged" or "kept". */
function ensureBinLink(home, binary) {
  const dir = path.join(home, "bin");
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  const link = path.join(dir, "sushiai");
  let current = null;
  try {
    current = fs.lstatSync(link);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  if (current && !current.isSymbolicLink()) return "kept";
  if (current && fs.readlinkSync(link) === binary) return "unchanged";
  const temp = `${link}.tmp-${process.pid}`;
  fs.rmSync(temp, { force: true });
  fs.symlinkSync(binary, temp);
  try {
    fs.renameSync(temp, link);
  } catch (error) {
    fs.rmSync(temp, { force: true });
    throw error;
  }
  return current ? "updated" : "created";
}

function daemonEnv(env, home) {
  const out = {};
  for (const key of DAEMON_ENV_KEYS) if (env[key]) out[key] = env[key];
  out.SUSHIAI_HOME = home;
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
} = {}) {
  const home = resolveHome(env);
  const socketPath = path.join(home, "daemon.sock");
  let binary = "";
  let bundledVersion = "";

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
    ensureBinLink(home, binary);
    try {
      await installHooksOnce();
    } catch (error) {
      // Hooks are not needed to run sessions; retried on the next connect.
      log(`hooks install failed: ${error.message}`);
    }
  }

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
      child.unref?.();
      return failed;
    } finally {
      fs.closeSync(fd);
    }
  }

  async function tryConnect() {
    try {
      return await connect({ socketPath, clientName: "desktop" });
    } catch {
      return null;
    }
  }

  async function connectWithin(ms, failed) {
    const deadline = Date.now() + ms;
    for (;;) {
      const client = await tryConnect();
      if (client) return client;
      if (Date.now() >= deadline)
        throw new Error(
          `sushiai daemon did not come up within ${Math.round(ms / 1000)} s (see ${path.join(home, "daemon.log")})`,
        );
      await Promise.race([sleep(pollMs), failed]);
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
    async connect() {
      await prepare();
      let client = await tryConnect();
      if (client && client.hello.daemon !== bundledVersion) {
        log(`daemon ${client.hello.daemon} != bundled ${bundledVersion}`);
        // Sessions live in their holders and survive the restart (D5).
        await client.request("daemon.shutdown", {}).catch(() => {});
        await waitGone(client);
        client = null;
      }
      if (!client) client = await connectWithin(startTimeoutMs, startDaemon());
      if (client.hello.daemon !== bundledVersion) {
        client.close();
        throw Object.assign(
          new Error(
            `sushiai daemon ${client.hello.daemon} does not match the bundled ${bundledVersion}`,
          ),
          { reason: "incompatible" },
        );
      }
      return client;
    },
  };
}

module.exports = {
  createLocalConnector,
  resolveHome,
  ensureHome,
  resolveBinary,
  ensureBinLink,
  HOOKS_STAMP,
};
