const { test } = require("node:test");
const assert = require("node:assert/strict");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const {
  OFF_MESSAGE,
  OrchestratorHosts,
  registerOrchestratorExtension,
} = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

const { appDb, closeAppDb } = require("../electron/app-db.cjs");

/** The wiring `registerOrchestratorExtension` needs, over a fake manager. */
async function setup(t, { on }) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "orch-toggle-"));
  const manager = fakeDaemonManager({
    handlers: { "orch.task.list": () => [{ id: "t1", title: "Remote job" }] },
  });
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
    getConnections: () => ({ get: () => ({}), list: () => [] }),
    getManager: () => manager,
    userDataDir: dir,
    hostsChanged: () => {},
  });
  t.after(async () => {
    await hosts.quit();
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  return { dir, manager, hosts, handlers, events, toggle };
}

test("enabling only listens: nothing is requested until the first call, and turning it off leaves the daemon running", async (t) => {
  const s = await setup(t, { on: true });
  await s.hosts.start();
  assert.equal(s.manager.listeners("event"), 1);
  assert.equal(s.manager.calls.length, 0);
  assert.deepEqual(await s.handlers["orchestrator-probe"]("local"), { pid: 0 });
  assert.equal(s.manager.calls.length, 0);

  const tasks = await s.handlers.orchestrator("task.list", {});
  assert.equal(tasks[0].title, "Remote job");
  assert.deepEqual(
    s.manager.calls.map((call) => [call.host, call.method]),
    [["local", "orch.task.list"]],
  );
  s.manager.notify("local", "orch.event", { event: "task", task: { id: "t" } });
  assert.equal(s.events.length, 1);

  // Off: listening stops and every entry point rejects without a request.
  await s.toggle(false);
  assert.equal(s.manager.listeners("event"), 0);
  for (const call of [
    () => s.handlers.orchestrator("task.list", {}),
    () => s.handlers["orchestrator-hosts"](),
    () => s.handlers["orchestrator-preflight"]("ssh:box"),
    () => s.handlers["orchestrator-host-setup"]("ssh:box"),
    () => s.handlers["orchestrator-probe"]("local"),
  ])
    await assert.rejects(async () => call(), { message: OFF_MESSAGE });
  assert.equal(s.manager.calls.length, 1);
  s.manager.notify("local", "orch.event", { event: "task", task: { id: "t" } });
  assert.equal(s.events.length, 1);

  // On again: listening resumes.
  await s.toggle(true);
  assert.equal(s.manager.listeners("event"), 1);
  s.manager.notify("local", "orch.event", { event: "task", task: { id: "u" } });
  assert.equal(s.events.length, 2);
});

test("a disabled orchestrator never listens or requests, whatever is asked", async (t) => {
  const s = await setup(t, { on: false });
  await s.hosts.start();
  await assert.rejects(async () => s.handlers.orchestrator("task.list", {}), {
    message: OFF_MESSAGE,
  });
  assert.equal(s.manager.listeners("event"), 0);
  assert.equal(s.manager.calls.length, 0);
  assert.equal(s.events.length, 0);
});

test("quitting the app leaves every daemon alone and only stops listening", async (t) => {
  const s = await setup(t, { on: true });
  await s.hosts.start();
  await s.handlers.orchestrator("task.list", {});
  await s.hosts.quit();
  assert.equal(s.manager.listeners("event"), 0);
  assert.equal(
    s.manager.calls.some((call) => !call.method.startsWith("orch.task")),
    false,
  );
});

test("turning it off forgets the remote services; turning it on lists the saved ones without connecting", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "orch-toggle-"));
  t.after(async () => {
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  appDb(dir)
    .prepare("INSERT INTO orchestrator_hosts(host) VALUES(?)")
    .run("ssh:box");
  const events = [];
  const manager = fakeDaemonManager({ hosts: { local: {} } });
  const hosts = new OrchestratorHosts({
    local: { refreshSecrets: async () => events.push("local.secrets") },
    connections: () => ({ get: () => ({}), list: () => [] }),
    getManager: () => manager,
    userDataDir: dir,
    onChange: () => {},
    createService: (host) => {
      events.push(`${host}.create`);
      return { refreshSecrets: async () => events.push(`${host}.secrets`) };
    },
  });
  await hosts.setEnabled(true);
  assert.deepEqual(events, [
    "ssh:box.create",
    "local.secrets",
    "ssh:box.secrets",
  ]);
  assert.equal(manager.calls.length, 0);
  await hosts.setEnabled(false);
  assert.equal(hosts.services.size, 0);
  await assert.rejects(hosts.call("task.list", {}, "ssh:box"), {
    message: OFF_MESSAGE,
  });
  // The saved list is left alone, so the next enable brings the host back.
  assert.deepEqual(
    appDb(dir)
      .prepare("SELECT host FROM orchestrator_hosts")
      .all()
      .map((row) => row.host),
    ["ssh:box"],
  );
  await hosts.setEnabled(true);
  assert.equal(events.filter((event) => event === "ssh:box.create").length, 2);
});

test("a host list that failed to load is not erased by a later save", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "orch-hosts-"));
  t.after(async () => {
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  const stored = () =>
    appDb(dir)
      .prepare("SELECT host FROM orchestrator_hosts ORDER BY host")
      .all()
      .map((row) => row.host);
  appDb(dir)
    .prepare("INSERT INTO orchestrator_hosts(host) VALUES(?)")
    .run("ssh:old");
  let known = false;
  const hosts = new OrchestratorHosts({
    local: {},
    getManager: () => null,
    connections: () => ({
      get: (host) => {
        if (!known && host === "ssh:old") throw new Error("not loaded yet");
        return {};
      },
      list: () => [],
    }),
    userDataDir: dir,
    onChange: () => {},
    createService: () => ({ call: async () => null }),
  });
  await hosts.init();
  await hosts.call("task.list", {}, "ssh:new");
  assert.deepEqual(stored(), ["ssh:new", "ssh:old"]);
});
