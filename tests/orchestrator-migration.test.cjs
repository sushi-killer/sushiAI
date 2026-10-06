// Cutover from a standalone orchd of a previous build: only the old process
// stops (electron/daemon/local.cjs stopLegacyOrchd). Its task data stays where
// it is; the orchestrator in the daemon starts clean.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  stopLegacyOrchd,
  createLocalConnector,
} = require("../electron/daemon/local.cjs");

const cleanups = [];
test.afterEach(async () => {
  while (cleanups.length) await cleanups.pop()();
});

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const until = async (check, ms = 5000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out");
};

/** A legacy data dir: pid, socket and token files plus a task that must stay. */
function legacyDir(pid) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "legacy-orchd-"));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  if (pid !== undefined)
    fs.writeFileSync(path.join(dir, "orchd.pid"), `${pid}`);
  fs.writeFileSync(path.join(dir, "orchd.sock"), "");
  fs.writeFileSync(path.join(dir, "control.token"), "old-token");
  fs.mkdirSync(path.join(dir, "tasks", "t1"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "tasks", "t1", "task.json"),
    JSON.stringify({ id: "t1", status: "running" }),
  );
  return dir;
}

/** A detached process that stands in for the old daemon: its own process
 * group, like the one the app spawned. */
function standIn({ ignoreTerm = false } = {}) {
  const script = ignoreTerm
    ? 'process.on("SIGTERM",()=>{});setInterval(()=>{},1000)'
    : 'process.on("SIGTERM",()=>process.exit(0));setInterval(()=>{},1000)';
  const child = spawn(process.execPath, ["-e", script], {
    detached: true,
    stdio: "ignore",
  });
  child.unref();
  cleanups.push(() => {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {}
  });
  return child;
}

const tasksStay = (dir) =>
  assert.equal(fs.existsSync(path.join(dir, "tasks", "t1", "task.json")), true);
const filesGone = (dir) => {
  for (const name of ["orchd.pid", "orchd.sock", "control.token"])
    assert.equal(fs.existsSync(path.join(dir, name)), false, name);
};

test("no pid file means nothing to stop", async () => {
  const dir = legacyDir();
  assert.equal(await stopLegacyOrchd(dir), "none");
  assert.equal(fs.existsSync(path.join(dir, "control.token")), true);
});

test("a pid file of a dead process is cleaned up and nothing is signalled", async () => {
  const child = standIn();
  const pid = child.pid;
  process.kill(-pid, "SIGKILL");
  await until(() => !alive(pid));
  const dir = legacyDir(pid);
  const signals = [];
  const result = await stopLegacyOrchd(dir, {
    kill: (target, signal) => {
      if (signal !== 0) signals.push(signal);
      return process.kill(target, signal);
    },
  });
  assert.equal(result, "stale");
  assert.deepEqual(signals, []);
  filesGone(dir);
  tasksStay(dir);
});

test("a pid that now belongs to another program is never signalled", async () => {
  const child = standIn();
  const dir = legacyDir(child.pid);
  const result = await stopLegacyOrchd(dir, { comm: () => "/usr/bin/vim" });
  assert.equal(result, "foreign");
  assert.equal(alive(child.pid), true);
  filesGone(dir);
  tasksStay(dir);
});

test("a running orchd gets SIGTERM, its files go and its tasks stay", async () => {
  const child = standIn();
  const dir = legacyDir(child.pid);
  const result = await stopLegacyOrchd(dir, {
    comm: () => "/old/bin/orchd",
    pollMs: 10,
  });
  assert.equal(result, "stopped");
  await until(() => !alive(child.pid));
  filesGone(dir);
  tasksStay(dir);
});

test("an orchd that ignores SIGTERM is killed with its process group after the timeout", async () => {
  const child = standIn({ ignoreTerm: true });
  const dir = legacyDir(child.pid);
  // Give the stand-in time to install its SIGTERM handler.
  await new Promise((resolve) => setTimeout(resolve, 300));
  const signals = [];
  const result = await stopLegacyOrchd(dir, {
    comm: () => "orchd",
    timeoutMs: 200,
    pollMs: 10,
    kill: (target, signal) => {
      if (signal !== 0) signals.push([target, signal]);
      return process.kill(target, signal);
    },
  });
  assert.equal(result, "stopped");
  assert.deepEqual(signals, [
    [child.pid, "SIGTERM"],
    [-child.pid, "SIGKILL"],
  ]);
  await until(() => !alive(child.pid));
  filesGone(dir);
});

test("the local connector stops a legacy orchd once per app run, before it runs anything of the new daemon", async () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "legacy-home-"));
  cleanups.push(() => fs.rmSync(home, { recursive: true, force: true }));
  const order = [];
  const connector = createLocalConnector({
    env: {
      SUSHIAI_HOME: path.join(home, "home"),
      SUSHIAI_DAEMON_BIN: path.join(home, "missing-sushiai"),
    },
    appVersion: "1.0.0",
    legacyOrchdDir: "/legacy/orchestrator",
    stopLegacy: async (dir) => order.push(["legacy", dir]),
    execFile: (_binary, args, _options, callback) => {
      order.push(["run", args[0]]);
      callback(new Error("no binary here"), "", "");
    },
  });
  await assert.rejects(connector.connect(), /no binary here|ENOENT/);
  await assert.rejects(connector.connect(), /no binary here|ENOENT/);
  assert.deepEqual(
    order.filter(([kind]) => kind === "legacy"),
    [["legacy", "/legacy/orchestrator"]],
  );
  assert.equal(order[0][0], "legacy");

  // A failing stop never blocks the daemon from starting.
  const failing = createLocalConnector({
    env: {
      SUSHIAI_HOME: path.join(home, "home2"),
      SUSHIAI_DAEMON_BIN: path.join(home, "missing-sushiai"),
    },
    appVersion: "1.0.0",
    legacyOrchdDir: "/legacy/orchestrator",
    stopLegacy: async () => {
      throw new Error("EPERM");
    },
    execFile: (_binary, _args, _options, callback) =>
      callback(new Error("no binary here"), "", ""),
  });
  await assert.rejects(failing.connect(), /no binary here|ENOENT/);
});
