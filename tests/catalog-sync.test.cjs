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
const {
  DEADLINE_MS,
  until,
  untilTurns,
  turn,
  pidStart,
  stopPid,
  removeDir,
} = require("./helpers/deadline.cjs");

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
        { endpoint: "/tmp/local.sock", cwd: "/work/alpha" },
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

test("groups.sync sends a host only the groups that belong to it", async () => {
  const manager = fakeManager({ local: "mac", box: "box-daemon" });
  const sync = createCatalogSync({
    manager,
    projects: fixtureProjects(),
    workspaces: async () => [
      { id: "w1", name: "Local", projectId: "" },
      { id: "w2", name: "Remote", projectId: "", connection: "ssh:box" },
      { id: "w3", name: "Local socket", connection: "/tmp/local.sock" },
      { id: "w4", name: "By project", projectId: "p2", connection: "ssh:gone" },
    ],
    store: memoryStore(),
  });
  const groupsFor = async (host) => {
    manager.calls.length = 0;
    manager.ready(host);
    await until(() => manager.calls.length === 2, `${host} calls`);
    return manager.calls[1].params.groups.map((g) => [g.id, g.order]);
  };
  assert.deepEqual(await groupsFor("local"), [
    ["w1", 0],
    ["w3", 2],
  ]);
  assert.deepEqual(await groupsFor("box"), [
    ["w2", 1],
    ["w4", 3],
  ]);
  sync.stop();
});

test("a change sends one debounced full sync to every ready host and rev grows", async (t) => {
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
  // The debounce runs on a mocked clock: the test advances it, so there is no
  // wall-clock margin to lose under load.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  projects.onChange();
  projects.onChange();
  sync.notifyChanged();
  assert.equal(manager.calls.length, 0, "debounced");
  t.mock.timers.tick(30);
  await untilTurns(() => manager.calls.length >= 4, "both hosts synced");
  // Any timer left over from the burst would fire in this much time. A late
  // sync is a few promise hops away, so one turn after the tick is enough to
  // see it; the final count then proves there was none.
  t.mock.timers.tick(10 * 60 * 1000);
  await turn();
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
    removeDir(dir);
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

const { realDaemon } = require("./helpers/real-daemon.cjs");
const { binary, skip } = realDaemon();
test(
  "the real daemon applies the sync and binds a session by cwd",
  { skip },
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
    let created;
    let holderPid;
    let holderStart;
    try {
      const socketPath = path.join(home, "daemon.sock");
      client = await until(async () => {
        if (!fs.existsSync(socketPath)) return undefined;
        return connectDaemon({
          socketPath,
          clientName: "catalog-test",
          helloTimeoutMs: DEADLINE_MS,
        }).catch(() => undefined);
      }, "the daemon socket");
      const manager = {
        ...fakeManager({ local: client.hello.host }),
        request: (_host, method, params) =>
          client.request(method, params, { timeoutMs: DEADLINE_MS }),
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
      created = await client.request(
        "session.create",
        {
          cmd: ["/bin/sh"],
          cwd: fs.realpathSync(folder),
          cols: 80,
          rows: 24,
          idempotencyKey: "catalog-sync-1",
        },
        { timeoutMs: DEADLINE_MS },
      );
      const listed = await client.request(
        "session.list",
        {},
        { timeoutMs: DEADLINE_MS },
      );
      const info = listed.find((s) => s.id === created.id);
      holderPid = info?.holderPid;
      if (holderPid) holderStart = pidStart(holderPid);
      assert.equal(info.project, "p1");
      sync.stop();
    } finally {
      // One ordered step: close the session and the client, stop the daemon
      // and the session's holder, wait for their exit, then remove the home.
      if (created) {
        await client
          ?.request(
            "session.close",
            { id: created.id, graceful: false },
            { timeoutMs: DEADLINE_MS },
          )
          .catch(() => undefined);
      }
      client?.close?.();
      await stopPid(daemon.pid);
      if (holderPid) await stopPid(holderPid, { start: holderStart });
      await exited;
      removeDir(home);
    }
  },
);
