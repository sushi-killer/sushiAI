const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { EventEmitter } = require("node:events");
const { spawn } = require("node:child_process");
const { connectDaemon } = require("../electron/daemon/client.cjs");
const { createCatalogSync, revStore } = require("../electron/catalog-sync.cjs");
const { closeAppDb } = require("../electron/app-db.cjs");

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check, what, ms = 5000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await wait(10);
  }
}

function fakeManager(hosts) {
  const emitter = new EventEmitter();
  const calls = [];
  const manager = {
    calls,
    failWith: null,
    states: () => Object.keys(hosts).map((host) => ({ host, state: "ready" })),
    hello: (host) => (hosts[host] ? { host: hosts[host] } : null),
    request: async (host, method, params) => {
      calls.push({ host, method, params });
      if (manager.failWith) throw manager.failWith;
      return { applied: 1, ignored: 0, rev: 1 };
    },
    on: (name, callback) => {
      emitter.on(name, callback);
      return () => emitter.off(name, callback);
    },
    ready: (host) => emitter.emit("state", { host, state: "ready" }),
  };
  return manager;
}

const fixtureProjects = () => ({
  onChange: null,
  list: async () => [
    {
      id: "p1",
      name: "Alpha",
      folders: [
        { endpoint: "herdr:sock", cwd: "/work/alpha" },
        { endpoint: "ssh:box", cwd: "/srv/alpha" },
      ],
    },
    { id: "p2", name: "Beta", folders: [{ endpoint: "ssh:box", cwd: "/b" }] },
  ],
});
const spaces = async () => [
  { id: "w1", name: "One", projectId: "p1" },
  { id: "w2", name: "Two", projectId: "" },
];
const memoryStore = () => {
  let rev = 0;
  return { get: () => rev, set: (value) => (rev = value) };
};

test("a ready host gets a full projects.sync with translated folders, then groups.sync", async () => {
  const manager = fakeManager({ local: "mac", box: "box-daemon" });
  const sync = createCatalogSync({
    manager,
    projects: fixtureProjects(),
    workspaces: spaces,
    store: memoryStore(),
  });
  manager.ready("local");
  await until(() => manager.calls.length === 2, "two calls");
  const [projectsCall, groupsCall] = manager.calls;
  assert.equal(projectsCall.host, "local");
  assert.equal(projectsCall.method, "projects.sync");
  assert.equal(projectsCall.params.host, "mac");
  assert.equal(projectsCall.params.full, true);
  assert.deepEqual(
    projectsCall.params.projects.map((p) => [p.id, p.folders]),
    [["p1", [{ host: "mac", path: "/work/alpha" }]]],
  );
  assert.equal(groupsCall.method, "groups.sync");
  assert.equal(groupsCall.params.full, true);
  assert.deepEqual(
    groupsCall.params.groups.map((g) => [g.id, g.projectId, g.name, g.order]),
    [
      ["w1", "p1", "One", 0],
      ["w2", "", "Two", 1],
    ],
  );
  manager.calls.length = 0;
  manager.ready("box");
  await until(() => manager.calls.length === 2, "box calls");
  assert.equal(manager.calls[0].params.host, "box-daemon");
  assert.deepEqual(
    manager.calls[0].params.projects.map((p) => [p.id, p.folders]),
    [
      ["p1", [{ host: "box-daemon", path: "/srv/alpha" }]],
      ["p2", [{ host: "box-daemon", path: "/b" }]],
    ],
  );
  sync.stop();
});

test("a change sends one debounced full sync to every ready host and rev grows", async () => {
  const manager = fakeManager({ local: "mac", box: "box-daemon" });
  const projects = fixtureProjects();
  const store = memoryStore();
  const sync = createCatalogSync({
    manager,
    projects,
    workspaces: spaces,
    store,
    debounceMs: 30,
  });
  manager.ready("local");
  await until(() => manager.calls.length === 2, "first sync");
  const first = manager.calls[0].params.projects[0].rev;
  manager.calls.length = 0;
  projects.onChange();
  projects.onChange();
  sync.notifyChanged();
  assert.equal(manager.calls.length, 0, "debounced");
  await until(() => manager.calls.length === 4, "both hosts");
  await wait(80);
  assert.equal(manager.calls.length, 4, "one burst, one sync per host");
  const hosts = manager.calls
    .filter((c) => c.method === "projects.sync")
    .map((c) => c.host)
    .sort();
  assert.deepEqual(hosts, ["box", "local"]);
  assert.ok(manager.calls[0].params.projects[0].rev > first);
  assert.ok(store.get() > first);
  sync.stop();
});

test("the rev persists across restarts of the sync", async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cs-"));
  t.after(() => {
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  });
  const run = async () => {
    const manager = fakeManager({ local: "mac" });
    const sync = createCatalogSync({
      manager,
      projects: fixtureProjects(),
      workspaces: spaces,
      store: revStore(dir),
    });
    manager.ready("local");
    await until(() => manager.calls.length === 2, "sync");
    sync.stop();
    return manager.calls[0].params.projects[0].rev;
  };
  const a = await run();
  const b = await run();
  assert.ok(b > a);
  assert.equal(revStore(dir).get(), b);
});

test("a daemon error is logged, not thrown", async () => {
  const manager = fakeManager({ local: "mac" });
  manager.failWith = Object.assign(new Error("daemon is other"), {
    code: -32602,
  });
  const logs = [];
  const sync = createCatalogSync({
    manager,
    projects: fixtureProjects(),
    workspaces: spaces,
    store: memoryStore(),
    log: (message) => logs.push(message),
  });
  manager.ready("local");
  await until(() => logs.length === 1, "a log line");
  assert.match(logs[0], /daemon is other/);
  await sync.syncHost("local");
  assert.equal(logs.length, 2);
  sync.stop();
});

const binary = path.join(
  process.env.CARGO_TARGET_DIR || path.join(__dirname, "../target"),
  "debug/sushiai",
);
test(
  "the real daemon applies the sync and binds a session by cwd",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async () => {
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "cs-"));
    fs.chmodSync(home, 0o700);
    const folder = path.join(home, "alpha");
    fs.mkdirSync(folder);
    const daemon = spawn(binary, ["daemon"], {
      env: {
        PATH: process.env.PATH,
        SUSHIAI_HOME: home,
        HOME: home,
        CODEX_HOME: path.join(home, "codex"),
        SUSHIAI_LOG: "off",
      },
      stdio: "ignore",
    });
    const exited = new Promise((resolve) => daemon.once("exit", resolve));
    let client;
    try {
      const socketPath = path.join(home, "daemon.sock");
      client = await until(async () => {
        if (!fs.existsSync(socketPath)) return undefined;
        return connectDaemon({ socketPath, clientName: "catalog-test" }).catch(
          () => undefined,
        );
      }, "the daemon socket");
      const manager = {
        ...fakeManager({ local: client.hello.host }),
        request: (_host, method, params) => client.request(method, params),
      };
      const logs = [];
      const sync = createCatalogSync({
        manager,
        projects: {
          list: async () => [
            {
              id: "p1",
              name: "Alpha",
              folders: [{ endpoint: "local", cwd: fs.realpathSync(folder) }],
            },
          ],
        },
        workspaces: async () => [{ id: "w1", name: "One", projectId: "p1" }],
        store: memoryStore(),
        log: (message) => logs.push(message),
      });
      await sync.syncHost("local");
      assert.deepEqual(logs, []);
      const catalog = path.join(home, "catalog.json");
      await until(
        () =>
          fs.existsSync(catalog) &&
          fs.readFileSync(catalog, "utf8").includes("Alpha"),
        "catalog.json",
      );
      const created = await client.request("session.create", {
        cmd: ["/bin/sh"],
        cwd: fs.realpathSync(folder),
        cols: 80,
        rows: 24,
        idempotencyKey: "catalog-sync-1",
      });
      const listed = await client.request("session.list", {});
      const info = listed.find((s) => s.id === created.id);
      assert.equal(info.project, "p1");
      sync.stop();
    } finally {
      client?.close?.();
      daemon.kill("SIGTERM");
      await Promise.race([exited, wait(3000)]);
      daemon.kill("SIGKILL");
      fs.rmSync(home, { recursive: true, force: true });
    }
  },
);
