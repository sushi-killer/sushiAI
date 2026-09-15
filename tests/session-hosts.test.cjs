const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/sessionHosts.ts");

const REMOTE = "example.test/dev/sushiai";
const profile = (id, name, extra = {}) => ({
  id,
  name,
  host: "192.0.2.10",
  socket: `ssh:${id}`,
  connected: true,
  ...extra,
});
const workspace = (id, connection, cwd, name = id) => ({
  id,
  name,
  cwd,
  connection,
  panels: [],
  layout: null,
});
const git = (remote, checkout, branch = "main", repo = checkout) => ({
  remote,
  commonDir: `${repo}/.git`,
  checkout,
  subdir: "",
  branch,
});
const gitFor = (workspaces, remote = REMOTE) =>
  Object.fromEntries(workspaces.map((w) => [w.id, git(remote, w.cwd)]));
const context = (
  workspaces,
  projectGit,
  connectionProfiles,
  workspaceGrouping,
) => ({
  workspaces,
  projectGit,
  connectionProfiles,
  workspaceGrouping,
});

test("workspace outside any merge group -> no host options", async () => {
  const { sessionHostOptions } = await library;
  const solo = workspace("w-solo", undefined, "/Users/dev/api");
  const options = sessionHostOptions(
    solo,
    context([solo], gitFor([solo]), [], "flat"),
  );
  assert.deepEqual(options, []);
});

test("grouped sidebar mode -> no host options even when a group would exist", async () => {
  const { sessionHostOptions } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const options = sessionHostOptions(
    local,
    context(
      [local, remote],
      gitFor([local, remote]),
      [profile("lab", "Lab")],
      "grouped",
    ),
  );
  assert.deepEqual(options, []);
});

test("local + remote host merge -> one option per host, labelled by host", async () => {
  const { sessionHostOptions } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const options = sessionHostOptions(
    local,
    context(
      [local, remote],
      gitFor([local, remote]),
      [profile("lab", "Lab")],
      "flat",
    ),
  );
  assert.deepEqual(options.map((o) => o.workspaceId).sort(), [
    "w-lab",
    "w-local",
  ]);
  const byId = Object.fromEntries(options.map((o) => [o.workspaceId, o]));
  assert.equal(byId["w-local"].label, "Local");
  assert.equal(byId["w-lab"].label, "Lab");
});

test("worktrees on one host -> labelled by branch alone, no repeated host", async () => {
  const { sessionHostOptions } = await library;
  const main = workspace("w-main", undefined, "/Users/dev/sushiai");
  const feature = workspace("w-feature", undefined, "/Users/dev/sushiai-fx");
  const projectGit = {
    "w-main": git(REMOTE, "/Users/dev/sushiai", "main"),
    "w-feature": git(
      REMOTE,
      "/Users/dev/sushiai-fx",
      "feature-x",
      "/Users/dev/sushiai",
    ),
  };
  const options = sessionHostOptions(
    main,
    context([main, feature], projectGit, [], "flat"),
  );
  const byId = Object.fromEntries(options.map((o) => [o.workspaceId, o]));
  assert.equal(byId["w-main"].label, "main");
  assert.equal(byId["w-feature"].label, "feature-x");
});

test("worktrees mixing hosts -> only the Local member needs the prefix added", async () => {
  const { sessionHostOptions } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remoteA = workspace("w-lab-a", "ssh:lab", "/home/dev/sushiai");
  const remoteB = workspace("w-lab-b", "ssh:lab", "/home/dev/sushiai-fx");
  const projectGit = {
    "w-local": git(REMOTE, "/Users/dev/sushiai", "main"),
    "w-lab-a": git(REMOTE, "/home/dev/sushiai", "main"),
    "w-lab-b": git(
      REMOTE,
      "/home/dev/sushiai-fx",
      "feature-x",
      "/home/dev/sushiai",
    ),
  };
  const options = sessionHostOptions(
    local,
    context(
      [local, remoteA, remoteB],
      projectGit,
      [profile("lab", "Lab")],
      "flat",
    ),
  );
  const byId = Object.fromEntries(options.map((o) => [o.workspaceId, o]));
  assert.equal(byId["w-local"].label, "Local · main");
  assert.equal(byId["w-lab-b"].label, "Lab · feature-x");
});

test("default selection (D1): the active workspace is always one of its own group's options", async () => {
  const { sessionHostOptions } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const options = sessionHostOptions(
    remote,
    context(
      [local, remote],
      gitFor([local, remote]),
      [profile("lab", "Lab")],
      "flat",
    ),
  );
  assert.ok(options.some((o) => o.workspaceId === remote.id));
});
