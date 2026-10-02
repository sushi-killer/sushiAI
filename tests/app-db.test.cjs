// The app database on node:sqlite: the JSON store imports, the projects.json import, the single
// folder resolver and the stored git identity behind projects:identify.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { makeStore, registerHandlers } = require("./helpers/fake-host.cjs");
const { appDb, closeAppDb } = require("../electron/app-db.cjs");
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
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 2);
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
      { id: "a", connection: "ssh:devbox", herdrId: "w1", cwd: "/repo/app" },
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

test("herdr-launches.json, connections.json and orchestrator-hosts.json are imported once", async (t) => {
  const dir = await tempDir(t);
  const record = {
    endpoint: "ssh:devbox",
    operationId: "op-1",
    created: { workspaceId: "w1", paneId: "p1", cwd: "/repo/app" },
    signatureHash: "s",
    preparationHash: "p",
  };
  const profile = (id) => ({
    id,
    name: id,
    host: "devbox",
    socket: "~/.sushiai/herdr.sock",
  });
  const one = "11111111-1111-1111-1111-111111111111";
  const two = "22222222-2222-2222-2222-222222222222";
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(
    path.join(dir, "herdr-launches.json"),
    JSON.stringify({ version: 1, operations: [record] }),
  );
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
    JSON.parse(db.prepare("SELECT data FROM launches").get().data),
    record,
  );
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
  for (const name of [
    "herdr-launches.json",
    "connections.json",
    "orchestrator-hosts.json",
  ]) {
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
    "herdr-launches.json",
    "connections.json",
    "orchestrator-hosts.json",
  ];
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  await fs.writeFile(path.join(dir, names[0]), "{broken");
  await fs.writeFile(path.join(dir, names[1]), "[1]");
  await fs.writeFile(path.join(dir, names[2]), '{"not":"a list"}');
  await fs.writeFile(path.join(dir, names[3]), "nope");
  const db = appDb(dir);
  for (const table of [
    "workspaces",
    "app_state",
    "launches",
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

test("invalid launch rows are dropped on load, valid ones kept", async (t) => {
  const dir = await tempDir(t);
  const { SessionLauncher } = require("../electron/session-launch.cjs");
  const warn = console.warn;
  console.warn = () => {};
  t.after(() => {
    console.warn = warn;
  });
  const db = appDb(dir);
  const good = {
    endpoint: "local",
    operationId: "ok",
    created: { workspaceId: "w1", paneId: "p1", cwd: "/repo/app" },
    signatureHash: "s",
    preparationHash: "p",
  };
  const insert = db.prepare(
    "INSERT INTO launches(endpoint, operation_id, data) VALUES(?, ?, ?)",
  );
  insert.run("local", "ok", JSON.stringify(good));
  insert.run("local", "bad", JSON.stringify({ endpoint: "local" }));
  insert.run("local", "junk", "{x");
  const launcher = new SessionLauncher({ userDataDir: dir });
  await launcher.loadJournal();
  assert.equal(launcher.journal.size, 1);
  assert.equal(count(db, "launches"), 1);
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
  const socket = "~/.sushiai/herdr.sock";
  const connections = new Connections(dir);
  await connections.init();
  const dev = await connections.save({ name: "Dev", host: "devbox", socket });
  const box = await connections.save({ name: "Box", host: "user@box", socket });
  await connections.setHidden(`ssh:${dev.id}`, true);
  await connections.delete(`ssh:${box.id}`);
  const again = await connections.save({
    name: "Box",
    host: "user@box2",
    socket,
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
