const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/workspaceMerge.ts");

// Invented hosts only, per AGENTS.md - no real hostnames or addresses.
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

test("two hosts + same remote + same basename -> one merged entry", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
  };
  const groups = computeMergeGroups([local, remote], remotes, [
    profile("lab", "Lab"),
  ]);
  assert.equal(groups.size, 2, "both member ids resolve to a group");
  assert.equal(groups.get("w-local"), groups.get("w-lab"));
  const group = groups.get("w-local");
  assert.equal(group.members.length, 2);
});

test("no git remote -> never merges", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/api");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/api");
  const remotes = { "w-local": "", "w-lab": "" };
  const groups = computeMergeGroups([local, remote], remotes, [
    profile("lab", "Lab"),
  ]);
  assert.equal(groups.size, 0);
});

test("same host twice -> never merges", async () => {
  const { computeMergeGroups } = await library;
  const a = workspace("w-a", "ssh:lab", "/home/dev/sushiai");
  const b = workspace("w-b", "ssh:lab", "/home/dev/sushiai");
  const remotes = {
    "w-a": "example.test/dev/sushiai",
    "w-b": "example.test/dev/sushiai",
  };
  const groups = computeMergeGroups([a, b], remotes, [profile("lab", "Lab")]);
  assert.equal(
    groups.size,
    0,
    "two workspaces on one host never merge with each other",
  );
});

test("same remote, different cwd basename -> does not merge", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const remote = workspace("w-lab", "ssh:lab", "/home/dev/sushiai-fork");
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
  };
  const groups = computeMergeGroups([local, remote], remotes, [
    profile("lab", "Lab"),
  ]);
  assert.equal(groups.size, 0);
});

test("member ordering: this Mac first, then remote hosts A->Z", async () => {
  const { computeMergeGroups } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const labB = workspace("w-labb", "ssh:labb", "/home/dev/sushiai");
  const labA = workspace("w-laba", "ssh:laba", "/home/dev/sushiai");
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-labb": "example.test/dev/sushiai",
    "w-laba": "example.test/dev/sushiai",
  };
  const profiles = [profile("laba", "Lab A"), profile("labb", "Lab B")];
  const groups = computeMergeGroups([local, labB, labA], remotes, profiles);
  const group = groups.get("w-local");
  assert.deepEqual(
    group.members.map((m) => m.workspace.id),
    ["w-local", "w-laba", "w-labb"],
  );

  // Three remote hosts, no local member: still sorted A->Z.
  const onlyRemotes = computeMergeGroups([labB, labA], remotes, profiles);
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
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
  };
  const group = computeMergeGroups([local, lab], remotes, [
    profile("lab", "Lab"),
  ]).get("w-local");

  // Active member wins even when it isn't This Mac.
  assert.equal(mergedRowStatusKey(group, "w-lab"), "ssh:lab");
  // No active member here -> falls back to This Mac.
  assert.equal(mergedRowStatusKey(group, "some-other-workspace"), LOCAL_GROUP);

  // With no local member, falls back to the first member in AC11 order.
  const labA = workspace("w-laba", "ssh:laba", "/home/dev/sushiai");
  const labB = workspace("w-labb", "ssh:labb", "/home/dev/sushiai");
  const remoteOnly = computeMergeGroups(
    [labA, labB],
    {
      "w-laba": "example.test/dev/sushiai",
      "w-labb": "example.test/dev/sushiai",
    },
    [profile("laba", "Lab A"), profile("labb", "Lab B")],
  ).get("w-laba");
  assert.equal(
    mergedRowStatusKey(remoteOnly, "no-active-workspace"),
    "ssh:laba",
  );
});

test("shouldCollapseHostMarkers: 3+ hosts always collapse, remote-label budget is 12 chars", async () => {
  const { computeMergeGroups, shouldCollapseHostMarkers } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
  };
  const twoHost = computeMergeGroups([local, lab], remotes, [
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
    {
      "w-local": "example.test/dev/sushiai",
      "w-laba": "example.test/dev/sushiai",
      "w-labb": "example.test/dev/sushiai",
    },
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
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
  };
  const profiles = [profile("lab", "Lab")];
  const group = computeMergeGroups([local, lab], remotes, profiles).get(
    "w-local",
  );
  const name = mergedMarkerAccessibleName(group, profiles, "/tmp/local.sock", {
    "/tmp/local.sock": "connected",
    "ssh:lab": "offline",
  });
  assert.equal(name, "Runs on This Mac (Connected) and Lab (Offline).");
});

test("activeMergedHostLabel: undefined in grouped mode, undefined when not merged, set for a merged member", async () => {
  const { activeMergedHostLabel } = await library;
  const local = workspace("w-local", undefined, "/Users/dev/sushiai");
  const lab = workspace("w-lab", "ssh:lab", "/home/dev/sushiai");
  const solo = workspace("w-solo", undefined, "/Users/dev/solo-project");
  const remotes = {
    "w-local": "example.test/dev/sushiai",
    "w-lab": "example.test/dev/sushiai",
    "w-solo": "",
  };
  const profiles = [profile("lab", "Lab")];
  const workspaces = [local, lab, solo];

  assert.equal(
    activeMergedHostLabel(workspaces, local, remotes, profiles, "grouped"),
    undefined,
    "merging is flat-mode only",
  );
  assert.equal(
    activeMergedHostLabel(workspaces, solo, remotes, profiles, "flat"),
    undefined,
    "a workspace with no merged group carries no host label",
  );
  assert.equal(
    activeMergedHostLabel(workspaces, lab, remotes, profiles, "flat"),
    "Lab",
  );
  assert.equal(
    activeMergedHostLabel(workspaces, local, remotes, profiles, "flat"),
    "This Mac",
  );
});
