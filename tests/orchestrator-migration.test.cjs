// Cutover from a standalone orchd of a previous build: only the old process
// stops (electron/orchestrator.cjs stopLegacyOrchd, run by `start()`). Its task
// data stays where it is; the orchestrator in the daemon starts clean. The first
// launch of this build turns the Orchestrator on only for an owner who used it.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  legacyOrchestratorPresent,
  registerOrchestratorExtension,
  stopLegacyOrchd,
} = require("../electron/orchestrator.cjs");
const { closeAppDb } = require("../electron/app-db.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

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

/** `registerOrchestratorExtension` over a fake manager and a fake extension
 * manager that records every `setEnabled`. Nothing touches the real home. */
function wire({
  enabled = false,
  saved = false,
  withLegacy = true,
  stopLegacy,
} = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "first-launch-"));
  const userDataDir = path.join(root, "userData");
  const homeDir = path.join(root, "home");
  fs.mkdirSync(userDataDir);
  fs.mkdirSync(homeDir);
  cleanups.push(() => {
    closeAppDb(userDataDir);
    fs.rmSync(root, { recursive: true, force: true });
  });
  const calls = [];
  const extensions = {
    ready: Promise.resolve(),
    enabled,
    isEnabled: () => extensions.enabled,
    hasSavedState: () => saved,
    onChange: () => {},
    setEnabled: async (id, value) => {
      calls.push([id, value]);
      extensions.enabled = value;
    },
  };
  const logs = [];
  const stops = [];
  const hosts = registerOrchestratorExtension({
    handle: () => {},
    extensions,
    send: () => {},
    getConnections: () => ({ get: () => ({}), list: () => [] }),
    getManager: () => fakeDaemonManager(),
    userDataDir,
    log: (message) => logs.push(message),
    hostsChanged: () => {},
    stopLegacy:
      stopLegacy ??
      (async (dir) => {
        stops.push(dir);
        return "stopped";
      }),
    legacy: withLegacy
      ? { homeDir, codexHome: path.join(homeDir, ".codex") }
      : undefined,
  });
  cleanups.push(() => hosts.quit());
  return { root, userDataDir, homeDir, calls, logs, stops, hosts };
}

test("start stops a legacy orchd of the app's own data dir, once per start", async () => {
  const w = wire();
  await w.hosts.start();
  assert.deepEqual(w.stops, [path.join(w.userDataDir, "orchestrator")]);
  assert.ok(w.logs.includes("stopped a legacy orchd"));
});

test("a legacy stop that fails never blocks start", async () => {
  const w = wire({
    stopLegacy: async () => {
      throw new Error("EPERM");
    },
  });
  await w.hosts.start();
  assert.deepEqual(w.logs, ["legacy orchd was not stopped: EPERM"]);
  assert.deepEqual(w.calls, [["builtin.orchestrator", false]]);
});

test("the first launch leaves a fresh install's Orchestrator off, once", async () => {
  const w = wire({ enabled: true, saved: true });
  await w.hosts.start();
  assert.deepEqual(w.calls, [["builtin.orchestrator", false]]);
  // The owner switches it on later: a restart must not undo that.
  w.calls.length = 0;
  await w.hosts.start();
  assert.deepEqual(w.calls, []);
});

test("legacy evidence never turns a saved off back on, and keeps a saved on", async () => {
  const off = wire({ saved: true, enabled: false });
  fs.mkdirSync(path.join(off.userDataDir, "orchestrator", "tasks"), {
    recursive: true,
  });
  await off.hosts.start();
  assert.deepEqual(off.calls, [["builtin.orchestrator", false]]);

  const on = wire({ saved: true, enabled: true });
  fs.mkdirSync(path.join(on.userDataDir, "orchestrator", "tasks"), {
    recursive: true,
  });
  await on.hosts.start();
  assert.deepEqual(on.calls, [["builtin.orchestrator", true]]);
});

test("the first launch turns the Orchestrator on for a legacy orchd data dir", async () => {
  const w = wire();
  fs.mkdirSync(path.join(w.userDataDir, "orchestrator", "tasks"), {
    recursive: true,
  });
  await w.hosts.start();
  assert.deepEqual(w.calls, [["builtin.orchestrator", true]]);
});

test("the first launch turns the Orchestrator on for a legacy MCP entry in Claude's or Codex's config", async () => {
  const claude = wire();
  fs.writeFileSync(
    path.join(claude.homeDir, ".claude.json"),
    JSON.stringify({
      mcpServers: { "sushiai-orchestrator": { command: "x" } },
    }),
  );
  await claude.hosts.start();
  assert.deepEqual(claude.calls, [["builtin.orchestrator", true]]);

  const codex = wire();
  fs.mkdirSync(path.join(codex.homeDir, ".codex"));
  fs.writeFileSync(
    path.join(codex.homeDir, ".codex", "config.toml"),
    '[mcp_servers.other]\ncommand = "o"\n\n[mcp_servers.sushiai-orchestrator]\ncommand = "x"\n',
  );
  await codex.hosts.start();
  assert.deepEqual(codex.calls, [["builtin.orchestrator", true]]);
});

test("another MCP entry is not a legacy Orchestrator", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "no-legacy-"));
  cleanups.push(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(
    path.join(root, ".claude.json"),
    JSON.stringify({ mcpServers: { other: { command: "o" } } }),
  );
  assert.equal(
    legacyOrchestratorPresent({ userDataDir: root, homeDir: root }),
    false,
  );
});
