const { test } = require("node:test");
const assert = require("node:assert/strict");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const { spawn } = require("node:child_process");
const {
  OFF_MESSAGE,
  OrchestratorHosts,
  registerOrchestratorExtension,
} = require("../electron/orchestrator.cjs");

const FAKE = path.join(__dirname, "fixtures", "fake-orchd.cjs");

async function waitUntil(check, { timeout = 3000, interval = 10 } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error("waitUntil: condition never became true");
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** A data dir and a repo root whose `orchd` binary is the fake daemon, plus
 * the wiring `registerOrchestratorExtension` needs. */
async function setup(t, { on }) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "orch-toggle-"));
  const root = path.join(dir, "root");
  const binary = path.join(root, "orchd", "target", "release", "orchd");
  await fs.mkdir(path.dirname(binary), { recursive: true });
  await fs.writeFile(
    binary,
    `#!/bin/sh\nexec "${process.execPath}" "${FAKE}" "$@"\n`,
    { mode: 0o755 },
  );
  const data = path.join(dir, "data");
  const pidFile = path.join(data, "orchd.pid");
  const readPid = async () =>
    Number(await fs.readFile(pidFile, "utf8").catch(() => 0));
  const listeners = [];
  const extensions = {
    ready: Promise.resolve(),
    on,
    isEnabled: () => extensions.on,
    onChange: (listener) => listeners.push(listener),
  };
  const toggle = async (value) => {
    extensions.on = value;
    await Promise.all(
      listeners.map((listener) => listener("builtin.orchestrator")),
    );
  };
  const handlers = {};
  const events = [];
  const hosts = registerOrchestratorExtension({
    handle: (channel, fn) => (handlers[channel] = fn),
    extensions,
    send: (channel, message) => events.push([channel, message]),
    dataDir: data,
    root,
    resourcesPath: root,
    packaged: false,
    getConnections: () => ({ get: () => ({}), list: () => [] }),
    hostsFile: path.join(dir, "hosts.json"),
    hostsChanged: () => {},
  });
  t.after(async () => {
    await hosts.quit();
    const pid = await readPid();
    if (pid && alive(pid)) process.kill(pid, "SIGKILL");
    await fs.rm(dir, { recursive: true, force: true });
  });
  return { dir, data, root, binary, hosts, handlers, events, toggle, readPid };
}

test("nothing starts at launch or on enable; the first request spawns orchd, turning it off stops it", async (t) => {
  const s = await setup(t, { on: true });
  await s.hosts.start();
  await new Promise((resolve) => setTimeout(resolve, 100));
  // Enabled, but no daemon was spawned and a probe never spawns one.
  assert.equal(await s.readPid(), 0);
  await assert.rejects(async () => s.handlers["orchestrator-probe"]("local"));
  assert.equal(await s.readPid(), 0);
  assert.equal(s.events.length, 0);

  // The first real request starts the daemon and the event relay.
  const tasks = await s.handlers.orchestrator("task.list", {});
  assert.equal(tasks[0].title, "Remote job");
  const pid = await s.readPid();
  assert.equal(alive(pid), true);
  await waitUntil(() =>
    s.events.some(([channel]) => channel === "orchestrator-event"),
  );
  assert.deepEqual(await s.handlers["orchestrator-probe"]("local"), {
    version: "fake",
    pid,
    dataDir: s.data,
  });

  // Off: the daemon this app spawned stops, the relay closes, no timer is
  // left, and every entry point rejects without touching the socket.
  await s.toggle(false);
  await waitUntil(() => !alive(pid));
  assert.equal(s.hosts.local.closed, true);
  assert.equal(s.hosts.local.retryTimer, null);
  for (const call of [
    () => s.handlers.orchestrator("task.list", {}),
    () => s.handlers["orchestrator-hosts"](),
    () => s.handlers["orchestrator-preflight"]("ssh:box"),
    () => s.handlers["orchestrator-host-setup"]("ssh:box"),
    () => s.handlers["orchestrator-probe"]("local"),
  ])
    await assert.rejects(async () => call(), { message: OFF_MESSAGE });
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(alive(pid), false);

  // On again, still lazy: a new request spawns a fresh daemon.
  await s.toggle(true);
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(alive(pid), false);
  await s.handlers.orchestrator("task.list", {});
  assert.notEqual(await s.readPid(), pid);
});

test("a disabled orchestrator never starts, whatever is asked", async (t) => {
  const s = await setup(t, { on: false });
  await s.hosts.start();
  await assert.rejects(async () => s.handlers.orchestrator("task.list", {}), {
    message: OFF_MESSAGE,
  });
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(await s.readPid(), 0);
  assert.equal(s.events.length, 0);
});

test("a daemon that is already running is attached without spawning, and quitting leaves it alone", async (t) => {
  const s = await setup(t, { on: true });
  await fs.mkdir(s.data, { recursive: true });
  const child = spawn(
    process.execPath,
    [
      FAKE,
      "serve",
      "--data",
      s.data,
      "--socket",
      path.join(s.data, "orchd.sock"),
    ],
    { stdio: "ignore" },
  );
  t.after(() => child.kill("SIGKILL"));
  await waitUntil(async () => (await s.readPid()) === child.pid);
  await waitUntil(() =>
    fs.access(path.join(s.data, "orchd.sock")).then(
      () => true,
      () => false,
    ),
  );

  await s.hosts.start();
  // Attached: its task event reaches the renderer, and nothing was spawned.
  await waitUntil(() =>
    s.events.some(([channel]) => channel === "orchestrator-event"),
  );
  assert.equal(await s.readPid(), child.pid);

  // This app did not start it, so quitting the app does not stop it.
  await s.hosts.quit();
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.equal(alive(child.pid), true);
});

test("quitting stops the local daemon this app spawned", async (t) => {
  const s = await setup(t, { on: true });
  await s.hosts.start();
  await s.handlers.orchestrator("task.list", {});
  const pid = await s.readPid();
  assert.equal(alive(pid), true);
  await s.hosts.quit();
  await waitUntil(() => !alive(pid));
});

test("turning it off stops the local daemon and quits every remote host; turning it on lists the saved ones without connecting", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "orch-toggle-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const hostsFile = path.join(dir, "hosts.json");
  await fs.writeFile(hostsFile, JSON.stringify(["ssh:box"]));
  const events = [];
  const hosts = new OrchestratorHosts({
    local: {
      attach: async () => events.push("local.attach"),
      close: () => events.push("local.close"),
      quit: async () => events.push("local.quit"),
    },
    connections: () => ({ get: () => ({}), list: () => [] }),
    hostsFile,
    onChange: () => {},
    createService: (host) => {
      events.push(`${host}.create`);
      return {
        connect: () => events.push(`${host}.connect`),
        quit: async () => events.push(`${host}.quit`),
      };
    },
  });
  await hosts.setEnabled(true);
  assert.deepEqual(events, ["local.attach", "ssh:box.create"]);
  await hosts.setEnabled(false);
  assert.deepEqual(events.slice(2), ["ssh:box.quit", "local.quit"]);
  assert.equal(hosts.services.size, 0);
  await assert.rejects(hosts.call("task.list", {}, "ssh:box"), {
    message: OFF_MESSAGE,
  });
  // The saved list is left alone, so the next enable brings the host back.
  assert.deepEqual(JSON.parse(await fs.readFile(hostsFile, "utf8")), [
    "ssh:box",
  ]);
  await hosts.setEnabled(true);
  assert.deepEqual(events.slice(4), ["local.attach", "ssh:box.create"]);
});

async function writeTask(data, id, task) {
  const dir = path.join(data, "tasks", id);
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(path.join(dir, "task.json"), JSON.stringify(task));
}

test("launch spawns orchd when a task on disk is still pending", async (t) => {
  const s = await setup(t, { on: true });
  await writeTask(s.data, "t1", { id: "t1", status: "done" });
  await writeTask(s.data, "t2", { id: "t2", status: "landing" });
  await s.hosts.start();
  const pid = await s.readPid();
  assert.ok(pid);
  assert.equal(alive(pid), true);
  await waitUntil(() =>
    s.events.some(([channel]) => channel === "orchestrator-event"),
  );
});

test("launch spawns nothing when every task on disk is finished or archived", async (t) => {
  const s = await setup(t, { on: true });
  await writeTask(s.data, "t1", { id: "t1", status: "done" });
  await writeTask(s.data, "t2", { id: "t2", status: "failed" });
  await writeTask(s.data, "t3", { id: "t3", status: "stopped" });
  await writeTask(s.data, "t4", { id: "t4", status: "queued", archived: true });
  await fs.mkdir(path.join(s.data, "tasks", "broken"), { recursive: true });
  await s.hosts.start();
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(await s.readPid(), 0);
});

test("launch spawns nothing for pending tasks while the orchestrator is off", async (t) => {
  const s = await setup(t, { on: false });
  await writeTask(s.data, "t1", { id: "t1", status: "running" });
  await s.hosts.start();
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.equal(await s.readPid(), 0);
});

test("quit waits for a daemon that needs seconds to stop, without SIGKILL", async (t) => {
  const s = await setup(t, { on: true });
  await fs.mkdir(s.data, { recursive: true });
  await fs.writeFile(path.join(s.data, "exit-delay"), "3000");
  await s.hosts.start();
  await s.handlers.orchestrator("task.list", {});
  const pid = await s.readPid();
  assert.equal(alive(pid), true);
  const kill = process.kill;
  const signals = [];
  process.kill = (target, signal) => {
    if (signal === "SIGKILL") signals.push(target);
    return kill(target, signal);
  };
  t.after(() => (process.kill = kill));
  await s.hosts.quit();
  process.kill = kill;
  assert.equal(alive(pid), false);
  assert.deepEqual(signals, []);
});
