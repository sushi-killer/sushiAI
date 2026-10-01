// Opening a project from any folder: by its remote (whatever it is called),
// else by the folder itself, and pulling what the folder holds into it.
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
const { remoteUrl } = require("../electron/git-remote.cjs");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");

async function repo(t, { remotes = {}, files = {}, config = [] } = {}) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "folder-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  for (const [name, url] of Object.entries(remotes))
    execFileSync("git", ["-C", dir, "remote", "add", name, url]);
  for (const [key, value] of config)
    execFileSync("git", ["-C", dir, "config", key, value]);
  for (const [name, text] of Object.entries(files))
    await fs.writeFile(path.join(dir, name), text);
  return dir;
}

async function harness(t) {
  const host = await makeHost(t);
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: new ClaudeMcp({ home: host.home }),
  });
  return { projects, call, host };
}

test("a folder with no remote gets a project of its own, the same one every time", async (t) => {
  const { projects, call } = await harness(t);
  const dir = await repo(t);
  const first = await call("projects:attach", {
    endpoint: "local",
    cwd: dir,
    name: "Notes",
  });
  assert.equal(first.name, "Notes");
  assert.equal(first.git.url, "");
  const again = await call("projects:attach", { cwd: dir, name: "Other" });
  assert.equal(again.id, first.id);
  assert.equal(again.name, "Notes");
  assert.equal((await projects.list()).length, 1);
  // Resolving it for the picker and the orchestrator finds it by folder.
  const found = await call("projects:resolve", { endpoint: "local", cwd: dir });
  assert.equal(found.id, first.id);
  // The same path on another host is another folder.
  const elsewhere = await projects.attach({
    endpoint: "ssh:lab",
    cwd: dir,
    name: "Notes on lab",
  });
  assert.notEqual(elsewhere.id, first.id);
});

test("a folder that gains a remote later keeps its project, which follows the remote", async (t) => {
  const { projects, call } = await harness(t);
  const dir = await repo(t);
  const first = await call("projects:attach", { cwd: dir, name: "App" });
  execFileSync("git", [
    "-C",
    dir,
    "remote",
    "add",
    "origin",
    "git@example.test:acme/app.git",
  ]);
  const later = await call("projects:attach", { cwd: dir, name: "App" });
  assert.equal(later.id, first.id);
  assert.equal(later.git.url, "git@example.test:acme/app.git");
  // Another checkout of the same remote joins that project.
  const other = await repo(t, {
    remotes: { origin: "https://example.test/acme/app" },
  });
  const joined = await call("projects:attach", { cwd: other, name: "Clone" });
  assert.equal(joined.id, first.id);
  assert.equal((await projects.folders(first.id)).length, 2);
});

test("the remote is found whatever it is called, and through insteadOf", async (t) => {
  const { host } = await harness(t);
  const upstream = await repo(t, {
    remotes: { upstream: "git@example.test:acme/app.git" },
  });
  const tracked = await repo(t, {
    remotes: {
      origin: "git@example.test:acme/fork.git",
      github: "git@example.test:acme/app.git",
    },
    config: [
      ["branch.main.remote", "github"],
      ["branch.main.merge", "refs/heads/main"],
    ],
  });
  const aliased = await repo(t, {
    remotes: { origin: "ex:acme/app" },
    config: [["url.https://example.test/.insteadOf", "ex:"]],
  });
  for (const [dir, expected] of [
    [upstream, "git@example.test:acme/app.git"],
    [tracked, "git@example.test:acme/app.git"],
    [aliased, "https://example.test/acme/app"],
  ]) {
    assert.equal(await remoteUrl(dir), expected);
    // The same answer from the inspection every dialog and picker asks.
    const seen = await host.connections.inspect(undefined, {
      operation: "git_remote",
      root: dir,
    });
    assert.equal(seen.remote, expected);
  }
  // And they all open the one project of that repository.
  const { projects, call } = await harness(t);
  const made = await projects.upsert({
    name: "App",
    git: { url: "https://example.test/acme/app", defaultBranch: "main" },
  });
  for (const dir of [upstream, tracked, aliased]) {
    const found = await call("projects:resolve", {
      endpoint: "local",
      cwd: dir,
    });
    assert.equal(found?.id, made.id, dir);
  }
});

test("a pull brings in new keys, fills empty values, and never brings back what was removed", async (t) => {
  const { projects, call } = await harness(t);
  const dir = await repo(t, {
    files: {
      ".env": "PORT=4000\nSTRIPE_SK=sk_live_invented0123\nEMPTY_ONE=\n",
      ".env.local": "LOG_LEVEL=debug\n",
      ".mcp.json": JSON.stringify({
        mcpServers: { docs: { command: "npx", args: ["docs"] } },
      }),
    },
  });
  const project = await call("projects:attach", { cwd: dir, name: "Envy" });
  const preview = await call("projects:import-local", project.id, dir, {
    preview: true,
  });
  assert.deepEqual(preview.newVariables.sort(), [
    "EMPTY_ONE",
    "LOG_LEVEL",
    "PORT",
    "STRIPE_SK",
  ]);
  assert.deepEqual(preview.newServers, ["docs"]);
  const first = await call("projects:import-local", project.id, dir);
  assert.equal(first.addedVariables.length, 4);
  assert.deepEqual(first.addedServers, ["docs"]);
  assert.equal(
    await projects.secretFor(project.id, "STRIPE_SK"),
    "sk_live_invented0123",
  );
  // Run again: nothing new, said plainly.
  const second = await call("projects:import-local", project.id, dir);
  assert.deepEqual(
    [second.addedVariables.length, second.addedServers.length],
    [0, 0],
  );
  assert.ok(second.skippedVariables.includes("PORT"));
  // A key that appears later is picked up; one the owner removed is not.
  await fs.appendFile(path.join(dir, ".env"), "NEW_KEY=1\n");
  await projects.updateEnv(project.id, { remove: ["LOG_LEVEL"] });
  const third = await call("projects:import-local", project.id, dir);
  assert.deepEqual(
    third.addedVariables.map((item) => item.name),
    ["NEW_KEY"],
  );
  assert.ok(third.removedVariables.includes("LOG_LEVEL"));
  assert.ok(!third.skippedVariables.includes("LOG_LEVEL"));
  // Asked for again by hand, it comes back.
  const forced = await call("projects:import-local", project.id, dir, {
    force: true,
  });
  assert.deepEqual(
    forced.addedVariables.map((item) => item.name),
    ["LOG_LEVEL"],
  );
  // A variable that exists without a value gets the folder's.
  await projects.updateEnv(project.id, {
    set: [{ name: "LATE_TOKEN", secret: true }],
  });
  await fs.appendFile(path.join(dir, ".env"), "LATE_TOKEN=now\n");
  const filled = await call("projects:import-local", project.id, dir);
  assert.deepEqual(filled.filledVariables, ["LATE_TOKEN"]);
  assert.equal(await projects.secretFor(project.id, "LATE_TOKEN"), "now");
});

test("a folder that is not the project's cannot be pulled into it", async (t) => {
  const { call } = await harness(t);
  const mine = await repo(t);
  const theirs = await repo(t, { files: { ".env": "STOLEN=1\n" } });
  const project = await call("projects:attach", { cwd: mine, name: "Mine" });
  await assert.rejects(
    call("projects:import-local", project.id, theirs),
    /not a checkout of this project/,
  );
});

test("a pull from a folder on an SSH host reads that host's files", async (t) => {
  const host = await makeHost(t);
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  const dir = path.join(host.home, "work", "app");
  await fs.mkdir(dir, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  await fs.writeFile(path.join(dir, ".env"), "REMOTE_KEY=1\n");
  const project = await call("projects:attach", {
    endpoint: host.endpoint,
    cwd: dir,
    name: "Remote notes",
  });
  assert.equal(project.git.url, "");
  const result = await call("projects:import-local", project.id, dir, {
    endpoint: host.endpoint,
  });
  assert.deepEqual(
    result.addedVariables.map((item) => item.name),
    ["REMOTE_KEY"],
  );
});

test("a host's checkout is found by its remote whatever the remote is called", async (t) => {
  const host = await makeHost(t);
  const { projects } = await makeStore(t);
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  const url = "git@example.test:acme/app.git";
  const project = await projects.upsert({
    name: "App",
    git: { url, defaultBranch: "main" },
  });
  const dir = path.join(host.home, "work", "app");
  await fs.mkdir(dir, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  execFileSync("git", ["-C", dir, "remote", "add", "upstream", url]);
  const ready = await call(
    "projects:host:check",
    project.id,
    host.endpoint,
    dir,
  );
  assert.equal(ready.checkout.ok, true);
});
