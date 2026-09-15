const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/workspaceMerge.ts");

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
/** A checkout's git identity; `repo` is the main checkout whose `.git` a
 * worktree shares (defaults to the checkout itself). */
const git = (remote, checkout, branch = "main", repo = checkout) => ({
  remote,
  commonDir: `${repo}/.git`,
  checkout,
  subdir: "",
  branch,
});
/** Every workspace checked out in its own cwd under one remote. */
const gitFor = (workspaces, remote = REMOTE) =>
  Object.fromEntries(workspaces.map((w) => [w.id, git(remote, w.cwd)]));

test("two hosts + same remote + same repository name -> one merged entry", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const groups = computeMergeGroups([local, remote], gitFor([local, remote]), [
    profile("lab", "Lab"),
  ]);
  assert.equal(groups.size, 2, "both member ids resolve to a group");
  assert.equal(groups.get("w-local"), groups.get("w-lab"));
  const group = groups.get("w-local");
  assert.equal(group.members.length, 2);
  assert.equal(group.worktrees, false);
});

test("no git remote -> never merges across hosts", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/api");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/api");
  const groups = computeMergeGroups(
    [local, remote],
    gitFor([local, remote], ""),
    [profile("lab", "Lab")],
  );
  assert.equal(groups.size, 0);
});

test("not a git repository -> never merges", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/api");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/api");
  const none = {
    remote: "",
    commonDir: "",
    checkout: "",
    subdir: "",
    branch: "",
  };
  const groups = computeMergeGroups(
    [local, remote],
    { "w-local": none, "w-lab": none },
    [profile("lab", "Lab")],
  );
  assert.equal(groups.size, 0);
});

test("one checkout opened twice on a host -> never merges", async () => {
  const { computeMergeGroups } = await library;
  const a = workspace("w-a", "ssh:lab", "/home/dev/sushiai");
  const b = workspace("w-b", "ssh:lab", "/home/dev/sushiai");
  const groups = computeMergeGroups([a, b], gitFor([a, b]), [
    profile("lab", "Lab"),
  ]);
  assert.equal(
    groups.size,
    0,
    "two workspaces in one checkout never merge with each other",
  );
});

test("same remote, different checkout folder names -> merges across hosts", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/AI Prototype");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/prototype");
  const groups = computeMergeGroups([local, remote], gitFor([local, remote]), [
    profile("lab", "Lab"),
  ]);
  assert.equal(groups.size, 2);
  assert.equal(groups.get("w-local"), groups.get("w-lab"));
});

test("normalizeRemote: one repository over SSH with a port, scp and HTTPS reads as one key", async () => {
  const { normalizeRemote } = await import("../src/app/useProjectGit.ts");
  const key = "git.example.test/team/prototype";
  for (const url of [
    "ssh://git@git.example.test:10022/team/prototype.git",
    "https://git.example.test/team/prototype.git",
    "https://deploy@git.example.test/team/prototype",
    "git@git.example.test:team/prototype.git",
    "HTTPS://git.example.test/Team/Prototype/",
  ])
    assert.equal(normalizeRemote(url), key, url);
  assert.notEqual(
    normalizeRemote("https://git.example.test/team/other.git"),
    key,
  );
});

test("worktrees of one repository on one host merge, labelled by branch, main checkout first", async () => {
  const { computeMergeGroups, memberLabel, mergedMarkerAccessibleName } =
    await library;
  const feature = workspace(
    "w-feature",
    undefined,
    "/Users/dev/sushiai-feature",
  );
  const main = workspace("w-main", undefined, "/Users/dev/sushiai");
  const projectGit = {
    "w-feature": git("", feature.cwd, "feature", main.cwd),
    "w-main": git("", main.cwd, "main"),
  };
  const group = computeMergeGroups([feature, main], projectGit, []).get(
    "w-feature",
  );
  assert.ok(group, "worktrees merge even without a remote");
  assert.equal(group.worktrees, true);
  assert.deepEqual(
    group.members.map((m) => m.workspace.id),
    ["w-main", "w-feature"],
  );
  assert.deepEqual(
    group.members.map((m) => memberLabel(group, m, [])),
    ["main", "feature"],
  );
  assert.equal(
    mergedMarkerAccessibleName(group, [], "/tmp/local.sock", {
      "/tmp/local.sock": "connected",
    }),
    "Checked out as main (Connected) and feature (Connected).",
  );
});

test("separate clones of one repository on one host never merge", async () => {
  const { computeMergeGroups } = await library;
  const a = workspace("w-a", undefined, "/Users/dev/sushiai");
  const b = workspace("w-b", undefined, "/Users/dev/copy/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const profiles = [profile("lab", "Lab")];
  assert.equal(computeMergeGroups([a, b], gitFor([a, b]), profiles).size, 0);
  assert.equal(
    computeMergeGroups([a, b, lab], gitFor([a, b, lab]), profiles).size,
    0,
    "a host holding two clones contributes neither, leaving Lab alone",
  );
});

test("a clone next to a worktree pair keeps the pair merged, and out of the cross-host row", async () => {
  const { computeMergeGroups, memberLabel } = await library;
  const main = workspace("w-main", undefined, "/Users/dev/sushiai");
  const feature = workspace(
    "w-feature",
    undefined,
    "/Users/dev/sushiai-feature",
  );
  const copy = workspace("w-copy", undefined, "/Users/dev/copy/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const profiles = [profile("lab", "Lab")];
  const projectGit = {
    "w-main": git(REMOTE, main.cwd),
    "w-feature": git(REMOTE, feature.cwd, "feature", main.cwd),
    "w-copy": git(REMOTE, copy.cwd),
    "w-lab": git(REMOTE, lab.cwd),
  };
  const groups = computeMergeGroups(
    [main, feature, copy, lab],
    projectGit,
    profiles,
  );
  const pair = groups.get("w-main");
  assert.ok(pair, "the worktrees still merge");
  assert.equal(groups.get("w-feature"), pair);
  assert.deepEqual(
    pair.members.map((m) => memberLabel(pair, m, profiles)),
    ["main", "feature"],
  );
  assert.equal(groups.has("w-copy"), false);
  assert.equal(groups.has("w-lab"), false, "Lab has no single match here");
});

test("different folders of one repository never merge across hosts", async () => {
  const { computeMergeGroups } = await library;
  const web = workspace("w-web", undefined, "/Users/dev/mono/apps/web");
  const api = workspace("w-api", "ssh:lab", "/home/dev/mono/apps/api");
  const projectGit = {
    "w-web": { ...git(REMOTE, "/Users/dev/mono"), subdir: "apps/web" },
    "w-api": { ...git(REMOTE, "/home/dev/mono"), subdir: "apps/api" },
  };
  assert.equal(
    computeMergeGroups([web, api], projectGit, [profile("lab", "Lab")]).size,
    0,
  );
  projectGit["w-api"].subdir = "apps/web";
  assert.equal(
    computeMergeGroups([web, api], projectGit, [profile("lab", "Lab")]).size,
    2,
  );
});

test("a worktree folder name does not block a cross-host merge", async () => {
  const { computeMergeGroups, memberLabel } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai-feature");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const profiles = [profile("lab", "Lab")];
  const projectGit = {
    "w-local": git(REMOTE, local.cwd, "feature", "/Users/dev/sushiai"),
    "w-lab": git(REMOTE, lab.cwd),
  };
  const group = computeMergeGroups([local, lab], projectGit, profiles).get(
    "w-lab",
  );
  assert.ok(group);
  assert.deepEqual(
    group.members.map((m) => memberLabel(group, m, profiles)),
    ["Local", "Lab"],
    "one checkout per host still reads as hosts",
  );
});

test("worktrees plus another host: labels carry the branch, the remote one its host too", async () => {
  const { computeMergeGroups, memberLabel, shouldCollapseHostMarkers } =
    await library;
  const main = workspace("w-main", undefined, "/Users/dev/sushiai");
  const feature = workspace(
    "w-feature",
    undefined,
    "/Users/dev/sushiai-feature",
  );
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const profiles = [profile("lab", "Lab")];
  const projectGit = {
    "w-main": git(REMOTE, main.cwd),
    "w-feature": git(REMOTE, feature.cwd, "feature", main.cwd),
    "w-lab": git(REMOTE, lab.cwd),
  };
  const group = computeMergeGroups(
    [lab, feature, main],
    projectGit,
    profiles,
  ).get("w-lab");
  assert.deepEqual(
    group.members.map((m) => memberLabel(group, m, profiles)),
    ["main", "feature", "Lab · main"],
  );
  assert.equal(shouldCollapseHostMarkers(group, profiles), true);

  const pair = computeMergeGroups([feature, main], projectGit, profiles).get(
    "w-main",
  );
  assert.equal(
    shouldCollapseHostMarkers(pair, profiles),
    false,
    "main + feature is 11 characters, within the budget",
  );
});

test("member ordering: this Mac first, then remote hosts A->Z", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const labB = workspace("w-labb", "ssh:labb", "/home/dev/sushiai");
  const labA = workspace("w-laba", "ssh:laba", "/home/dev/sushiai");
  const projectGit = gitFor([local, labB, labA]);
  const profiles = [profile("laba", "Lab A"), profile("labb", "Lab B")];
  const groups = computeMergeGroups([local, labB, labA], projectGit, profiles);
  const group = groups.get("w-local");
  assert.deepEqual(
    group.members.map((m) => m.workspace.id),
    ["w-local", "w-laba", "w-labb"],
  );

  const onlyRemotes = computeMergeGroups([labB, labA], projectGit, profiles);
  const remoteGroup = onlyRemotes.get("w-laba");
  assert.deepEqual(
    remoteGroup.members.map((m) => m.workspace.id),
    ["w-laba", "w-labb"],
  );
});

test("D2: the status dot follows the active member, else this Mac, else the first", async () => {
  const { computeMergeGroups, mergedRowStatusKey, LOCAL_GROUP } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const group = computeMergeGroups([local, lab], gitFor([local, lab]), [
    profile("lab", "Lab"),
  ]).get("w-local");

  assert.equal(mergedRowStatusKey(group, "w-lab"), "ssh:lab");
  assert.equal(mergedRowStatusKey(group, "some-other-workspace"), LOCAL_GROUP);

  const labA = workspace("w-laba", "ssh:laba", "/home/dev/sushiai");
  const labB = workspace("w-labb", "ssh:labb", "/home/dev/sushiai");
  const remoteOnly = computeMergeGroups([labA, labB], gitFor([labA, labB]), [
    profile("laba", "Lab A"),
    profile("labb", "Lab B"),
  ]).get("w-laba");
  assert.equal(
    mergedRowStatusKey(remoteOnly, "no-active-workspace"),
    "ssh:laba",
  );
});

test("shouldCollapseHostMarkers: 3+ hosts always collapse, remote-label budget is 12 chars", async () => {
  const { computeMergeGroups, shouldCollapseHostMarkers } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const twoHost = computeMergeGroups([local, lab], gitFor([local, lab]), [
    profile("lab", "Lab"),
  ]).get("w-local");
  assert.equal(
    shouldCollapseHostMarkers(twoHost, [profile("lab", "Lab")]),
    false,
  );

  const longLabelProfiles = [profile("lab", "production-eu-west")];
  assert.equal(
    shouldCollapseHostMarkers(twoHost, longLabelProfiles),
    true,
    "18 remote characters exceeds the 12-character budget",
  );

  const labA = workspace("w-laba", "ssh:laba", "/home/dev/sushiai");
  const labB = workspace("w-labb", "ssh:labb", "/home/dev/sushiai");
  const threeHost = computeMergeGroups(
    [local, labA, labB],
    gitFor([local, labA, labB]),
    [profile("laba", "A"), profile("labb", "B")],
  ).get("w-local");
  assert.equal(
    shouldCollapseHostMarkers(threeHost, [
      profile("laba", "A"),
      profile("labb", "B"),
    ]),
    true,
    "3+ hosts collapse regardless of label length",
  );
});

test("mergedMarkerAccessibleName: two and three hosts, no Oxford comma", async () => {
  const { computeMergeGroups, mergedMarkerAccessibleName } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const profiles = [profile("lab", "Lab")];
  const group = computeMergeGroups(
    [local, lab],
    gitFor([local, lab]),
    profiles,
  ).get("w-local");
  const name = mergedMarkerAccessibleName(group, profiles, "/tmp/local.sock", {
    "/tmp/local.sock": "connected",
    "ssh:lab": "offline",
  });
  assert.equal(name, "Runs on Local (Connected) and Lab (Offline).");
});
