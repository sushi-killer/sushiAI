// Remotes that are never overwritten, strict attach, stable and unique slugs
// and what an import may and may not touch.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  makeHost,
  makeStore,
  registerHandlers,
} = require("./helpers/fake-host.cjs");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");

const LAB = "ssh:lab";

async function repo(t, remotes = {}, files = {}) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "grant-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  for (const [name, url] of Object.entries(remotes))
    execFileSync("git", ["-C", dir, "remote", "add", name, url]);
  for (const [name, text] of Object.entries(files))
    await fs.writeFile(path.join(dir, name), text);
  return dir;
}

test("a folder with another remote does not rewrite the project's remote", async (t) => {
  const { projects } = await makeStore(t);
  const first = await projects.attach({
    remote: "git@example.test:acme/a.git",
    endpoint: "local",
    cwd: "/work/a",
    name: "A",
  });
  // The same folder is later seen with a different remote B.
  const again = await projects.attach({
    remote: "git@example.test:acme/b.git",
    endpoint: "local",
    cwd: "/work/a",
    name: "A",
  });
  assert.equal(again.id, first.id);
  assert.equal(again.git.url, "git@example.test:acme/a.git");
  assert.equal(
    (await projects.get(first.id)).git.url,
    "git@example.test:acme/a.git",
  );
});

test("a folder that cannot be asked about its remote is an error, not a project without one", async (t) => {
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: {
      inspect: async () => {
        throw new Error("host unreachable");
      },
    },
    claudeMcp: {},
  });
  await assert.rejects(
    call("projects:attach", { endpoint: LAB, cwd: "/srv/app" }),
    /host unreachable/,
  );
  assert.equal((await projects.list()).length, 0);
});

test("the remote that wins is the tracked one, then origin, then the only one", async (t) => {
  const { remoteUrl } = require("../electron/git-remote.cjs");
  const onlyUpstream = await repo(t, { upstream: "git@example.test:a/up.git" });
  assert.equal(await remoteUrl(onlyUpstream), "git@example.test:a/up.git");
  const both = await repo(t, {
    upstream: "git@example.test:a/up.git",
    origin: "git@example.test:a/or.git",
  });
  assert.equal(await remoteUrl(both), "git@example.test:a/or.git");
});

test("a project's slug is stored at creation and never follows a rename", async (t) => {
  const { projects } = await makeStore(t);
  const made = await projects.upsert({ name: "My App" });
  assert.equal(made.slug, "my-app");
  const renamed = await projects.upsert({ id: made.id, name: "Other Name" });
  assert.equal(renamed.slug, "my-app");
  assert.equal((await projects.get(made.id)).slug, "my-app");
  // A project stored before slugs existed gets the one its name gives, once.
  const file = path.join(projects.projectsFile);
  const stored = JSON.parse(await fs.readFile(file, "utf8"));
  delete stored[made.id].slug;
  stored[made.id].name = "Legacy Name";
  await fs.writeFile(file, JSON.stringify(stored));
  const legacy = await projects.upsert({ id: made.id, name: "Renamed Later" });
  assert.equal(legacy.slug, "legacy-name");
});

test("an import never returns values, stays on its endpoint, and fills only secrets", async (t) => {
  const host = await makeHost(t);
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: new ClaudeMcp({ home: host.home }),
  });
  const dir = await repo(
    t,
    {},
    { ".env": "PLAIN_EMPTY=from-folder\nSECRET_EMPTY=sk_live_invented0123\n" },
  );
  const project = await call("projects:attach", { cwd: dir, name: "Envy" });
  await projects.updateEnv(project.id, {
    set: [
      { name: "PLAIN_EMPTY", secret: false, value: "" },
      { name: "SECRET_EMPTY", secret: true },
    ],
  });
  const result = await call("projects:import-local", project.id, dir);
  assert.ok(!JSON.stringify(result).includes("sk_live_invented0123"));
  assert.ok(!JSON.stringify(result).includes("from-folder"));
  assert.deepEqual(result.filledVariables, ["SECRET_EMPTY"]);
  assert.equal(
    await projects.secretFor(project.id, "SECRET_EMPTY"),
    "sk_live_invented0123",
  );
  // The same path on another host is not the attached folder.
  await assert.rejects(
    call("projects:import-local", project.id, dir, { endpoint: LAB }),
    /./,
  );
});

test("a preview counts what is new, and names what was removed apart from it", async (t) => {
  const host = await makeHost(t);
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: new ClaudeMcp({ home: host.home }),
  });
  const dir = await repo(t, {}, { ".env": "KEEP=1\nGONE=2\n" });
  const project = await call("projects:attach", { cwd: dir, name: "Envy" });
  await call("projects:import-local", project.id, dir);
  await projects.updateEnv(project.id, { remove: ["GONE"] });
  const preview = await call("projects:import-local", project.id, dir, {
    preview: true,
  });
  assert.deepEqual(preview.newVariables, []);
  assert.deepEqual(preview.removedVariables, ["GONE"]);
  // An ordinary pull does not bring it back; only force does, by name.
  const plain = await call("projects:import-local", project.id, dir);
  assert.deepEqual(plain.addedVariables, []);
  const forced = await call("projects:import-local", project.id, dir, {
    force: true,
  });
  assert.deepEqual(
    forced.addedVariables.map((item) => item.name),
    ["GONE"],
  );
});

test("two projects with one name get two folders on a host", async (t) => {
  const { projects } = await makeStore(t);
  const first = await projects.upsert({ name: "My App" });
  const second = await projects.upsert({ name: "My App" });
  const attached = await projects.attach({
    endpoint: "local",
    cwd: "/work/my-app",
    name: "My App",
  });
  assert.deepEqual(
    [first.slug, second.slug, attached.slug],
    ["my-app", "my-app-2", "my-app-3"],
  );
});

test("creating a project twice from one repository is one project", async (t) => {
  const { projects } = await makeStore(t);
  const first = await projects.upsert({
    name: "App",
    git: { url: "git@example.test:acme/app.git", defaultBranch: "main" },
  });
  // The same repository spelled another way, from another window or a double
  // click: the same project, with what it already has.
  const again = await projects.upsert({
    name: "App copy",
    git: { url: "https://example.test/acme/app", defaultBranch: "main" },
  });
  assert.equal(again.id, first.id);
  assert.equal(again.name, "App");
  assert.equal((await projects.list()).length, 1);
  // A different repository is a different project.
  const other = await projects.upsert({
    name: "Other",
    git: { url: "git@example.test:acme/other.git", defaultBranch: "main" },
  });
  assert.notEqual(other.id, first.id);
  // Concurrent creates of one repository still make one.
  const both = await Promise.all([
    projects.upsert({
      name: "N",
      git: { url: "git@example.test:acme/new.git" },
    }),
    projects.upsert({
      name: "N",
      git: { url: "git@example.test:acme/new.git" },
    }),
  ]);
  assert.equal(both[0].id, both[1].id);
});
