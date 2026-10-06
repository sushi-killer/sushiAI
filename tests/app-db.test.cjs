// The app database on node:sqlite: the JSON store imports, the projects.json import, the single
// folder resolver and the stored git identity behind projects:identify.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { makeStore, registerHandlers } = require("./helpers/fake-host.cjs");
const {
  DAMAGED,
  appDb,
  closeAppDb,
  readStore,
  writeStore,
} = require("../electron/app-db.cjs");
const { Connections } = require("../electron/connections.cjs");
const {
  readSnapshot,
  savedWorkspaces,
} = require("../electron/workspace-snapshot.cjs");

const project = (id, url, folders = []) => ({
  id,
  name: id,
  slug: id,
  git: { url, defaultBranch: "" },
  env: [],
  mcp: {},
  setup: { install: "", check: "" },
  network: { allowedDomains: [] },
  sessions: {},
  targets: ["local"],
  hosts: {},
  folders,
  dismissed: { env: [], mcp: [] },
});

async function tempDir(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-db-"));
  t.after(async () => {
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  return dir;
}

const exists = (file) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

test("first open imports projects.json with its folders and renames it", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "projects.json");
  await fs.writeFile(
    file,
    JSON.stringify({
      a: project("a", "git@example.test:team/app.git", [
        { endpoint: "local", cwd: "/repo/app" },
        { endpoint: "ssh:devbox", cwd: "/srv/app" },
      ]),
      b: project("b", ""),
    }),
  );
  const { Projects } = require("../electron/projects.cjs");
  const projects = new Projects({ userDataDir: dir, safeStorage: {} });
  const list = await projects.list();
  assert.deepEqual(
    list.map((item) => item.id),
    ["a", "b"],
  );
  assert.deepEqual(list[0].folders, [
    { endpoint: "local", cwd: "/repo/app" },
    { endpoint: "ssh:devbox", cwd: "/srv/app" },
  ]);
  assert.equal(await exists(file), false);
  assert.equal(await exists(`${file}.imported`), true);
  assert.equal(
    (await projects.get("a")).git.url,
    "git@example.test:team/app.git",
  );
});

test("a failed import rolls back, keeps the file, never blocks opening and retries on the next open", async (t) => {
  const dir = await tempDir(t);
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  const file = path.join(dir, "projects.json");
  await fs.writeFile(file, '{"a": {"name": "a"}, "b": ');
  assert.equal(count(appDb(dir), "projects"), 0);
  assert.equal(await exists(file), true);
  assert.equal(await exists(`${file}.imported`), false);
  closeAppDb(dir);
  await fs.writeFile(file, JSON.stringify({ a: project("a", "") }));
  const db = appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(db.prepare("SELECT count(*) AS n FROM projects").get().n, 1);
  assert.equal(await exists(`${file}.imported`), true);
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 5);
  assert.equal(
    db.prepare("PRAGMA journal_mode").get().journal_mode.toLowerCase(),
    "wal",
  );
});

test("an import that fails midway leaves no rows behind", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "projects.json");
  await fs.writeFile(
    file,
    JSON.stringify({
      a: project("a", ""),
      b: { folders: 5 },
      c: project("c", ""),
    }),
  );
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  assert.equal(count(appDb(dir), "projects"), 0);
  closeAppDb(dir);
  await fs.writeFile(file, JSON.stringify({ a: project("a", "") }));
  const db = appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(db.prepare("SELECT count(*) AS n FROM projects").get().n, 1);
});

test("the resolver prefers an attached folder, then the remote, then a shared common dir", async (t) => {
  const { projects } = await makeStore(t);
  const first = await projects.attach({
    remote: "git@example.test:team/app.git",
    endpoint: "local",
    cwd: "/repo/app",
    commonDir: "/repo/app/.git",
  });
  const second = await projects.attach({
    remote: "git@example.test:team/other.git",
    endpoint: "local",
    cwd: "/repo/other",
  });
  assert.notEqual(first.id, second.id);
  // The attached folder wins over a remote that belongs to another project.
  assert.equal(
    (
      await projects.resolveProject({
        host: "local",
        cwd: "/repo/other",
        remoteKey: "example.test/team/app",
      })
    ).id,
    second.id,
  );
  // An unattached folder follows its remote.
  assert.equal(
    (
      await projects.resolveProject({
        host: "local",
        cwd: "/repo/new",
        remoteKey: "example.test/team/app",
      })
    ).id,
    first.id,
  );
  // No remote: a folder of the same repository on the same host.
  assert.equal(
    (
      await projects.resolveProject({
        host: "local",
        cwd: "/repo/worktree",
        commonDir: "/repo/app/.git",
      })
    ).id,
    first.id,
  );
  assert.equal(
    await projects.resolveProject({
      host: "ssh:devbox",
      cwd: "/repo/worktree",
      commonDir: "/repo/app/.git",
    }),
    null,
  );
  assert.equal(
    await projects.resolveProject({ host: "local", cwd: "/x" }),
    null,
  );
});

test("attach joins the project of an attached folder even when its remote changed", async (t) => {
  const { projects } = await makeStore(t);
  const made = await projects.attach({
    endpoint: "local",
    cwd: "/repo/app",
  });
  const again = await projects.attach({
    remote: "git@example.test:team/app.git",
    endpoint: "local",
    cwd: "/repo/app",
  });
  assert.equal(again.id, made.id);
  assert.equal(again.git.url, "git@example.test:team/app.git");
  assert.equal((await projects.list()).length, 1);
});

test("deleting a project unattaches its folders but keeps what was identified", async (t) => {
  const { projects } = await makeStore(t);
  const made = await projects.attach({
    remote: "git@example.test:team/app.git",
    endpoint: "local",
    cwd: "/repo/app",
  });
  projects.rememberFolder("local", "/repo/app", {
    remote: "example.test/team/app",
    commonDir: "/repo/app/.git",
    checkout: "/repo/app",
    linkedWorktree: false,
    subdir: "",
    branch: "main",
  });
  await projects.delete(made.id);
  assert.deepEqual(await projects.list(), []);
  const rows = projects.db.prepare("SELECT * FROM folders").all();
  assert.equal(rows.length, 1);
  assert.equal(rows[0].project_id, null);
  assert.equal(rows[0].branch, "main");
  assert.equal(
    await projects.resolveProject({ host: "local", cwd: "/repo/app" }),
    null,
  );
});

test("identify reads git once per run, then answers from the store", async (t) => {
  const { projects } = await makeStore(t);
  const made = await projects.upsert({
    name: "App",
    git: { url: "git@example.test:team/app.git" },
  });
  let online = true;
  let reads = 0;
  let branch = "main";
  const connections = {
    inspect: async (host, request) => {
      reads += 1;
      if (!online) throw new Error("host unreachable");
      assert.deepEqual(request, { operation: "git_remote", root: "/repo/app" });
      assert.equal(host, "ssh:devbox");
      return {
        remote: "git@example.test:team/app.git",
        commonDir: "/repo/app/.git",
        checkout: "/repo/app",
        linkedWorktree: false,
        subdir: "",
        branch,
      };
    },
  };
  const call = registerHandlers({ projects, connections });
  const live = await call("projects:identify", "ssh:devbox", "/repo/app");
  assert.deepEqual(live, {
    projectId: made.id,
    remote: "example.test/team/app",
    commonDir: "/repo/app/.git",
    checkout: "/repo/app",
    linkedWorktree: false,
    subdir: "",
    branch: "main",
    stale: false,
  });
  // The identity was stored without attaching anything.
  assert.deepEqual(await projects.folders(made.id), []);
  const seenAt = projects.db.prepare("SELECT seen_at FROM folders").get();

  // A stored folder is answered from the store without asking git again.
  const cached = await call("projects:identify", "ssh:devbox", "/repo/app");
  assert.equal(reads, 1);
  assert.equal(cached.projectId, made.id);
  assert.equal(cached.stale, true);

  // `refresh` reads git; an unchanged identity writes nothing.
  await call("projects:identify", "ssh:devbox", "/repo/app", { refresh: true });
  assert.equal(reads, 2);
  assert.deepEqual(
    projects.db.prepare("SELECT seen_at FROM folders").get(),
    seenAt,
  );
  branch = "feature";
  const moved = await call("projects:identify", "ssh:devbox", "/repo/app", {
    refresh: true,
  });
  assert.equal(moved.branch, "feature");
  assert.equal(moved.stale, false);

  online = false;
  const offline = await call("projects:identify", "ssh:devbox", "/repo/app", {
    refresh: true,
  });
  assert.equal(offline.stale, true);
  assert.equal(offline.projectId, made.id);
  assert.equal(offline.branch, "feature");
  const inside = await call(
    "projects:identify",
    "ssh:devbox",
    "/repo/app/packages/ui",
  );
  assert.equal(inside.stale, true);
  assert.equal(inside.subdir, "packages/ui");
  assert.equal(inside.checkout, "/repo/app");
  assert.equal(inside.projectId, made.id);
  // A sibling that only shares the prefix is not inside the checkout.
  await assert.rejects(
    call("projects:identify", "ssh:devbox", "/repo/app-two"),
    /unreachable/,
  );
  // Another host has nothing stored.
  await assert.rejects(
    call("projects:identify", "ssh:other", "/repo/app"),
    /unreachable/,
  );
  await assert.rejects(call("projects:identify", "local", ""), /folder/);
  await assert.rejects(call("projects:identify", "local", 3), /folder/);
});

test("a new run refreshes a stored identity in the background", async (t) => {
  const { projects } = await makeStore(t);
  projects.rememberFolder("ssh:devbox", "/repo/app", {
    remote: "example.test/team/old",
    commonDir: "/repo/app/.git",
    checkout: "/repo/app",
    linkedWorktree: false,
    subdir: "",
    branch: "main",
  });
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const connections = {
    inspect: async () => {
      await gate;
      return {
        remote: "git@example.test:team/app.git",
        commonDir: "/repo/app/.git",
        checkout: "/repo/app",
        linkedWorktree: false,
        subdir: "",
        branch: "main",
      };
    },
  };
  const call = registerHandlers({ projects, connections });
  // Answered at once from the store, while git is still being read.
  const first = await call("projects:identify", "ssh:devbox", "/repo/app");
  assert.equal(first.remote, "example.test/team/old");
  release();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    projects.storedFolder("ssh:devbox", "/repo/app").remote,
    "example.test/team/app",
  );
});

test("a checkout of a fork joins the project whose attached folder tracks it", async (t) => {
  const { projects } = await makeStore(t);
  const made = await projects.attach({
    remote: "git@example.test:team/app.git",
    endpoint: "local",
    cwd: "/repo/app",
  });
  // The local folder now tracks a fork; the project keeps its own remote.
  projects.rememberFolder("local", "/repo/app", {
    remote: "example.test/me/app",
    commonDir: "/repo/app/.git",
    checkout: "/repo/app",
    linkedWorktree: false,
    subdir: "",
    branch: "main",
  });
  assert.equal(
    projects.projectIdFor({
      host: "ssh:devbox",
      cwd: "/srv/app",
      remoteKey: "example.test/me/app",
    }),
    made.id,
  );
  // A caller that only names the folder gets its stored identity filled in.
  projects.rememberFolder("local", "/repo/app-wt", {
    remote: "",
    commonDir: "/repo/app/.git",
    checkout: "/repo/app-wt",
    linkedWorktree: true,
    subdir: "",
    branch: "topic",
  });
  assert.equal(
    (await projects.resolveFolder({ endpoint: "local", cwd: "/repo/app-wt" }))
      ?.id,
    made.id,
  );
});

const count = (db, table) =>
  db.prepare(`SELECT count(*) AS n FROM ${table}`).get().n;

test("workspace-state.json splits into workspace rows and app state, then is renamed", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "workspace-state.json");
  const snapshot = {
    activeId: "a",
    views: { a: { zoom: 1 } },
    workspaces: [
      { id: "a", connection: "ssh:devbox", cwd: "/repo/app" },
      { id: "b", cwd: "/repo/other" },
    ],
  };
  await fs.writeFile(file, JSON.stringify(snapshot));
  const db = appDb(dir);
  assert.equal(count(db, "workspaces"), 2);
  assert.equal(count(db, "app_state"), 2);
  assert.deepEqual(JSON.parse(readSnapshot(dir)), snapshot);
  assert.deepEqual(
    savedWorkspaces(dir).map((item) => item.id),
    ["a", "b"],
  );
  assert.equal(await exists(file), false);
  assert.equal(await exists(`${file}.imported`), true);
});

test("v3 to v4 drops the old external-id column and keeps rows, order and endpoint", async (t) => {
  const dir = await tempDir(t);
  const first = appDb(dir);
  // Put the database back the way v3 left it.
  first.exec(`
    DROP TABLE workspaces;
    CREATE TABLE workspaces(
      id TEXT PRIMARY KEY,
      position INTEGER NOT NULL,
      endpoint TEXT NOT NULL DEFAULT '',
      legacy_id TEXT,
      data TEXT NOT NULL
    );
    CREATE UNIQUE INDEX workspaces_legacy ON workspaces(endpoint, legacy_id)
      WHERE legacy_id IS NOT NULL AND legacy_id != '';
    PRAGMA user_version = 3;
  `);
  const insert = first.prepare(
    "INSERT INTO workspaces(id, position, endpoint, legacy_id, data) VALUES(?, ?, ?, ?, ?)",
  );
  insert.run("b", 1, "", null, JSON.stringify({ id: "b", cwd: "/repo/b" }));
  insert.run(
    "a",
    0,
    "ssh:devbox",
    "w1",
    JSON.stringify({ id: "a", connection: "ssh:devbox" }),
  );
  closeAppDb(dir);
  const db = appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 5);
  assert.deepEqual(
    db
      .prepare("PRAGMA table_info(workspaces)")
      .all()
      .map((c) => c.name),
    ["id", "position", "endpoint", "data"],
  );
  assert.equal(
    db
      .prepare(
        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'workspaces_legacy'",
      )
      .get(),
    undefined,
  );
  assert.deepEqual(
    db
      .prepare(
        "SELECT id, position, endpoint FROM workspaces ORDER BY position",
      )
      .all()
      .map((row) => ({ ...row })),
    [
      { id: "a", position: 0, endpoint: "ssh:devbox" },
      { id: "b", position: 1, endpoint: "" },
    ],
  );
  assert.deepEqual(
    savedWorkspaces(dir).map((item) => item.id),
    ["a", "b"],
  );
});

test("a v3 database is copied to sushiai.db.v3.bak once, before migration 4 runs", async (t) => {
  const dir = await tempDir(t);
  const first = appDb(dir);
  first.exec(`
    DROP TABLE workspaces;
    CREATE TABLE workspaces(
      id TEXT PRIMARY KEY,
      position INTEGER NOT NULL,
      endpoint TEXT NOT NULL DEFAULT '',
      legacy_id TEXT,
      data TEXT NOT NULL
    );
    PRAGMA user_version = 3;
  `);
  first
    .prepare(
      "INSERT INTO workspaces(id, position, endpoint, legacy_id, data) VALUES('a', 0, '', 'w1', '{}')",
    )
    .run();
  closeAppDb(dir);
  const backup = path.join(dir, "sushiai.db.v3.bak");
  await assert.rejects(fs.stat(backup));
  const db = appDb(dir);
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 5);
  const { DatabaseSync } = require("node:sqlite");
  const old = new DatabaseSync(backup);
  assert.equal(old.prepare("PRAGMA user_version").get().user_version, 3);
  assert.equal(
    old.prepare("SELECT legacy_id FROM workspaces").get().legacy_id,
    "w1",
  );
  old.close();
  // A later open never replaces the copy.
  closeAppDb(dir);
  await fs.writeFile(backup, "kept");
  appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(await fs.readFile(backup, "utf8"), "kept");
});

test("a fresh database makes no v3 backup", async (t) => {
  const dir = await tempDir(t);
  appDb(dir);
  t.after(() => closeAppDb(dir));
  await assert.rejects(fs.stat(path.join(dir, "sushiai.db.v3.bak")));
});

test("a fresh database has the plain workspaces table and no launches table", async (t) => {
  const dir = await tempDir(t);
  const db = appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 5);
  assert.deepEqual(
    db
      .prepare("PRAGMA table_info(workspaces)")
      .all()
      .map((column) => column.name),
    ["id", "position", "endpoint", "data"],
  );
  assert.equal(
    db.prepare("SELECT 1 FROM sqlite_master WHERE name = 'launches'").get(),
    undefined,
  );
});

test("v4 to v5 drops the launches table of the retired backend", async (t) => {
  const dir = await tempDir(t);
  const first = appDb(dir);
  first.exec(`
    CREATE TABLE launches(endpoint TEXT, operation_id TEXT, data TEXT);
    PRAGMA user_version = 4;
  `);
  closeAppDb(dir);
  const db = appDb(dir);
  t.after(() => closeAppDb(dir));
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 5);
  assert.equal(
    db.prepare("SELECT 1 FROM sqlite_master WHERE name = 'launches'").get(),
    undefined,
  );
});

test("connections.json and orchestrator-hosts.json are imported once", async (t) => {
  const dir = await tempDir(t);
  const profile = (id) => ({
    id,
    name: id,
    host: "devbox",
  });
  const one = "11111111-1111-1111-1111-111111111111";
  const two = "22222222-2222-2222-2222-222222222222";
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(
    path.join(dir, "connections.json"),
    JSON.stringify([profile(one), { name: "no host" }, profile(two)]),
  );
  await fs.writeFile(
    path.join(dir, "orchestrator-hosts.json"),
    JSON.stringify(["ssh:devbox"]),
  );
  const db = appDb(dir);
  assert.deepEqual(
    db
      .prepare("SELECT id FROM connections ORDER BY position")
      .all()
      .map((row) => row.id),
    [one, two],
  );
  const loaded = new Connections(dir);
  await loaded.init();
  t.after(() => loaded.close());
  assert.equal(loaded.list().length, 2);
  assert.equal(count(db, "orchestrator_hosts"), 1);
  for (const name of ["connections.json", "orchestrator-hosts.json"]) {
    assert.equal(await exists(path.join(dir, name)), false);
    assert.equal(await exists(path.join(dir, `${name}.imported`)), true);
  }
  // A second file never overwrites a filled table.
  closeAppDb(dir);
  await fs.writeFile(path.join(dir, "orchestrator-hosts.json"), '["ssh:x"]');
  assert.equal(count(appDb(dir), "orchestrator_hosts"), 1);
  assert.equal(await exists(path.join(dir, "orchestrator-hosts.json")), true);
});

test("an invalid legacy file is left in place and never blocks opening", async (t) => {
  const dir = await tempDir(t);
  const names = [
    "workspace-state.json",
    "connections.json",
    "orchestrator-hosts.json",
  ];
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(path.join(dir, names[0]), "{broken");
  await fs.writeFile(path.join(dir, names[1]), '{"not":"a list"}');
  await fs.writeFile(path.join(dir, names[2]), "nope");
  const db = appDb(dir);
  for (const table of [
    "workspaces",
    "app_state",
    "connections",
    "orchestrator_hosts",
  ])
    assert.equal(count(db, table), 0);
  for (const name of names) {
    assert.equal(await exists(path.join(dir, name)), true);
    assert.equal(await exists(path.join(dir, `${name}.imported`)), false);
  }
});

test("a broken projects.json does not stop the other imports", async (t) => {
  const dir = await tempDir(t);
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(path.join(dir, "projects.json"), "{broken");
  await fs.writeFile(
    path.join(dir, "workspace-state.json"),
    JSON.stringify({ activeId: "a", workspaces: [{ id: "a" }] }),
  );
  const db = appDb(dir);
  assert.equal(count(db, "projects"), 0);
  assert.equal(count(db, "workspaces"), 1);
  assert.equal(await exists(path.join(dir, "projects.json")), true);
  assert.equal(
    await exists(path.join(dir, "workspace-state.json.imported")),
    true,
  );
});

test("appDb returns one connection per directory", async (t) => {
  const dir = await tempDir(t);
  assert.equal(appDb(dir), appDb(dir));
  const first = appDb(dir);
  closeAppDb(dir);
  assert.notEqual(appDb(dir), first);
});

test("connection profiles persist across a reopen", async (t) => {
  const dir = await tempDir(t);
  const connections = new Connections(dir);
  await connections.init();
  const dev = await connections.save({ name: "Dev", host: "devbox" });
  const box = await connections.save({ name: "Box", host: "user@box" });
  await connections.setHidden(`ssh:${dev.id}`, true);
  await connections.delete(`ssh:${box.id}`);
  const again = await connections.save({
    name: "Box",
    host: "user@box2",
  });
  await connections.close();
  closeAppDb(dir);
  const reopened = new Connections(dir);
  await reopened.init();
  t.after(() => reopened.close());
  assert.deepEqual(
    reopened.list().map((item) => [item.id, item.host, item.hidden]),
    [
      [dev.id, "devbox", true],
      [again.id, "user@box2", false],
    ],
  );
});

// [store, legacy file, legacy text, file deleted after import]
const STORE_FILES = [
  ["providers", "providers.json", '{"p1":{"id":"p1"}}', false],
  [
    "secrets",
    "secrets.json",
    '{"p1":{"v":1,"backend":"plain","ct":"AAAA"},"__proto__":{"v":1,"ct":"CCCC"}}',
    true,
  ],
  ["model-profiles", "model-profiles.json", '{"m1":{"id":"m1"}}', false],
  ["claude-accounts", "claude-accounts.json", '{"a1":{"id":"a1"}}', true],
  ["codex-accounts", "codex-accounts.json", '{"c1":{"id":"c1"}}', true],
  ["project-secrets", "project-secrets.json", '{"p:TOKEN":{"ct":"BB"}}', true],
  ["window", "window-state.json", '{"bounds":{"x":1}}', false],
  ["updates", "updates.json", '{"autoCheck":false}', false],
  ["preferences", "app-preferences.json", '{"notifications":false}', false],
];
const DOCUMENTS = ["window", "updates", "preferences"];

test("each settings file is imported into its store once; secret files are deleted, the rest renamed", async (t) => {
  const dir = await tempDir(t);
  for (const [, file, text] of STORE_FILES)
    await fs.writeFile(path.join(dir, file), text);
  appDb(dir);
  for (const [name, file, text, removed] of STORE_FILES) {
    const legacy = JSON.parse(text);
    const expected = DOCUMENTS.includes(name) ? { value: legacy } : legacy;
    assert.deepEqual({ ...readStore(dir, name) }, { ...expected }, name);
    assert.equal(await exists(path.join(dir, file)), false, file);
    assert.equal(await exists(path.join(dir, `${file}.imported`)), !removed);
  }
  assert.equal(Object.hasOwn(readStore(dir, "secrets"), "__proto__"), true);
  assert.equal(Object.keys(readStore(dir, "secrets")).length, 2);
  assert.equal(
    {}.v,
    undefined,
    "a __proto__ key never reaches Object.prototype",
  );
  closeAppDb(dir);
  await fs.writeFile(path.join(dir, "providers.json"), '{"p2":{"id":"p2"}}');
  appDb(dir);
  assert.deepEqual(Object.keys(readStore(dir, "providers")), ["p1"]);
  assert.equal(await exists(path.join(dir, "providers.json")), true);
});

test("a settings file that is not an object or not JSON is kept and does not block opening", async (t) => {
  const dir = await tempDir(t);
  const warned = [];
  const warn = console.warn;
  console.warn = (message) => warned.push(String(message));
  t.after(() => {
    console.warn = warn;
  });
  for (const [, file] of STORE_FILES)
    await fs.writeFile(
      path.join(dir, file),
      file.startsWith("s") || file.startsWith("u") ? "[]" : "{fixture-text",
    );
  appDb(dir);
  for (const [name, file] of STORE_FILES) {
    assert.deepEqual({ ...readStore(dir, name) }, {}, name);
    assert.equal(await exists(path.join(dir, file)), true, file);
    assert.equal(await exists(path.join(dir, `${file}.imported`)), false);
  }
  assert.ok(warned.length > 0);
  assert.equal(
    warned.some((line) => line.includes("fixture-text")),
    false,
  );
});

test("writeStore writes only changed keys and deletes missing ones", async (t) => {
  const dir = await tempDir(t);
  const db = appDb(dir);
  const changes = () => db.prepare("SELECT total_changes() AS n").get().n;
  writeStore(dir, "demo", { a: { n: 1 }, b: { n: 2 }, c: 3 });
  const start = changes();
  writeStore(dir, "demo", { a: { n: 1 }, b: { n: 5 } });
  assert.equal(changes() - start, 2, "one update and one delete");
  assert.deepEqual({ ...readStore(dir, "demo") }, { a: { n: 1 }, b: { n: 5 } });
  const idle = changes();
  writeStore(dir, "demo", { a: { n: 1 }, b: { n: 5 } });
  assert.equal(changes(), idle, "an identical write touches nothing");
  writeStore(dir, "other", { z: 1 });
  writeStore(dir, "demo", {});
  assert.deepEqual({ ...readStore(dir, "demo") }, {});
  assert.deepEqual({ ...readStore(dir, "other") }, { z: 1 });
});

test("an unreadable row is skipped by readStore and never deleted by writeStore", async (t) => {
  const dir = await tempDir(t);
  const warned = [];
  const warn = console.warn;
  console.warn = (message) => warned.push(String(message));
  t.after(() => {
    console.warn = warn;
  });
  writeStore(dir, "demo", { ok: 1 });
  const db = appDb(dir);
  db.prepare(
    "INSERT INTO store(name, key, value) VALUES('demo', 'bad', '{fixture-text')",
  ).run();
  assert.deepEqual({ ...readStore(dir, "demo") }, { ok: 1 });
  assert.equal(
    warned.some((line) => line.includes("fixture-text")),
    false,
  );
  writeStore(dir, "demo", { ok: 2 });
  assert.equal(
    db
      .prepare("SELECT value FROM store WHERE name = 'demo' AND key = 'bad'")
      .get().value,
    "{fixture-text",
  );
  assert.deepEqual({ ...readStore(dir, "demo") }, { ok: 2 });
});

test("surface, extension, scheduler and journal files are imported and renamed", async (t) => {
  const dir = await tempDir(t);
  const files = {
    "extensions/state/local.tasks.json": '{"board":{"1":{"/a":[1]}}}',
    "extensions/extensions.json": '{"schemaVersion":2}',
    "extensions/extension-lock.json": '{"schemaVersion":2,"packages":{}}',
    "agents/hermes-scheduler.json": '{"enabled":true}',
    "agents/hermes-activity.json": '[{"id":"a","title":"t","createdAt":1}]',
  };
  for (const [file, text] of Object.entries(files)) {
    await fs.mkdir(path.dirname(path.join(dir, file)), { recursive: true });
    await fs.writeFile(path.join(dir, file), text);
  }
  appDb(dir);
  assert.deepEqual(readStore(dir, "surface-state")["local.tasks"], {
    board: { 1: { "/a": [1] } },
  });
  assert.equal(readStore(dir, "extensions").value.schemaVersion, 2);
  assert.deepEqual(readStore(dir, "extension-lock").value.packages, {});
  assert.equal(readStore(dir, "hermes-scheduler").enabled, true);
  assert.deepEqual(readStore(dir, "hermes-activity").order, ["a"]);
  for (const file of Object.keys(files)) {
    assert.equal(await exists(path.join(dir, file)), false, file);
    assert.equal(await exists(path.join(dir, `${file}.imported`)), true, file);
  }
});

test("a damaged extension file leaves a damaged row, stays in place and is retried until repaired", async (t) => {
  const dir = await tempDir(t);
  const warned = [];
  const warn = console.warn;
  console.warn = (message) => warned.push(String(message));
  t.after(() => {
    console.warn = warn;
  });
  const files = {
    "extensions/extensions.json": ["extensions", "value"],
    "extensions/extension-lock.json": ["extension-lock", "value"],
    "extensions/state/local.tasks.json": ["surface-state", "local.tasks"],
  };
  for (const file of Object.keys(files)) {
    await fs.mkdir(path.dirname(path.join(dir, file)), { recursive: true });
    await fs.writeFile(path.join(dir, file), "{fixture-text");
  }
  appDb(dir);
  for (const [file, [name, key]] of Object.entries(files)) {
    assert.equal(readStore(dir, name, { damaged: true })[key], DAMAGED, file);
    assert.equal(Object.hasOwn(readStore(dir, name), key), false);
    assert.equal(await exists(path.join(dir, file)), true, file);
  }
  assert.equal(
    warned.some((line) => line.includes("fixture-text")),
    false,
  );
  closeAppDb(dir);
  appDb(dir);
  assert.equal(readStore(dir, "extensions", { damaged: true }).value, DAMAGED);
  closeAppDb(dir);
  await fs.writeFile(
    path.join(dir, "extensions/extensions.json"),
    '{"schemaVersion":2}',
  );
  await fs.writeFile(
    path.join(dir, "extensions/state/local.tasks.json"),
    '{"board":{}}',
  );
  appDb(dir);
  assert.deepEqual(readStore(dir, "extensions").value, { schemaVersion: 2 });
  assert.deepEqual(readStore(dir, "surface-state")["local.tasks"], {
    board: {},
  });
  assert.equal(
    await exists(path.join(dir, "extensions/extensions.json.imported")),
    true,
  );
  assert.equal(
    readStore(dir, "extension-lock", { damaged: true }).value,
    DAMAGED,
    "the still-damaged lock file stays damaged",
  );
});

test("a secret file is removed only after its import committed", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "secrets.json");
  await fs.writeFile(file, '{"p1":{"ct":"AAAA"}}');
  appDb(dir);
  closeAppDb(dir);
  assert.equal(await exists(file), false);
  assert.equal(await exists(`${file}.imported`), false);
  // A renamed copy whose removal failed is removed on the next open.
  await fs.writeFile(`${file}.imported`, '{"p1":{"ct":"AAAA"}}');
  appDb(dir);
  closeAppDb(dir);
  assert.equal(await exists(`${file}.imported`), false);
  // A file that was never imported (the store already holds data) stays.
  await fs.writeFile(file, '{"p2":{"ct":"BBBB"}}');
  appDb(dir);
  assert.equal(await exists(file), true);
});

test("a damaged secret file is kept even after the store fills", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "codex-accounts.json");
  await fs.writeFile(file, '{"a":1,}');
  appDb(dir);
  writeStore(dir, "codex-accounts", { b: { label: "B" } });
  closeAppDb(dir);
  appDb(dir);
  assert.equal(await exists(file), true);
});

test("removing a damaged extension file clears its marker", async (t) => {
  const dir = await tempDir(t);
  await fs.mkdir(path.join(dir, "extensions", "state"), { recursive: true });
  const settings = path.join(dir, "extensions", "extensions.json");
  const surface = path.join(dir, "extensions", "state", "acme.board.json");
  await fs.writeFile(settings, "{oops");
  await fs.writeFile(surface, "{oops");
  appDb(dir);
  assert.equal(readStore(dir, "extensions", { damaged: true }).value, DAMAGED);
  assert.equal(
    readStore(dir, "surface-state", { damaged: true })["acme.board"],
    DAMAGED,
  );
  closeAppDb(dir);
  await fs.rm(settings);
  await fs.rm(surface);
  appDb(dir);
  assert.deepEqual(
    Object.keys(readStore(dir, "extensions", { damaged: true })),
    [],
  );
  assert.deepEqual(
    Object.keys(readStore(dir, "surface-state", { damaged: true })),
    [],
  );
});
