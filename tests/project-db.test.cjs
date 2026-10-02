// The project store on node:sqlite: the projects.json import, the single
// folder resolver and the stored git identity behind projects:identify.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { makeStore, registerHandlers } = require("./helpers/fake-host.cjs");
const { openProjectDb } = require("../electron/project-db.cjs");

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
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
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

test("a failed import rolls back, keeps the file and retries on the next open", async (t) => {
  const dir = await tempDir(t);
  const file = path.join(dir, "projects.json");
  await fs.writeFile(file, '{"a": {"name": "a"}, "b": ');
  assert.throws(() => openProjectDb(dir));
  assert.equal(await exists(file), true);
  assert.equal(await exists(`${file}.imported`), false);
  await fs.writeFile(file, JSON.stringify({ a: project("a", "") }));
  const db = openProjectDb(dir);
  t.after(() => db.close());
  assert.equal(db.prepare("SELECT count(*) AS n FROM projects").get().n, 1);
  assert.equal(await exists(`${file}.imported`), true);
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, 1);
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
  assert.throws(() => openProjectDb(dir));
  await fs.writeFile(file, JSON.stringify({ a: project("a", "") }));
  const db = openProjectDb(dir);
  t.after(() => db.close());
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
