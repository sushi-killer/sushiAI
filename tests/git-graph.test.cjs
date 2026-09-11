const { test } = require("node:test");
const assert = require("node:assert/strict");

test("linear history stays in a single lane", async () => {
  const { computeGitGraph } = await import("../src/gitGraph.ts");
  const commits = [
    { hash: "C3", parents: ["C2"], author: "a", date: 3, subject: "third", refs: [] },
    { hash: "C2", parents: ["C1"], author: "a", date: 2, subject: "second", refs: [] },
    { hash: "C1", parents: [], author: "a", date: 1, subject: "first", refs: [] },
  ];
  const layout = computeGitGraph(commits);
  assert.equal(layout.laneCount, 1);
  assert.deepEqual(layout.nodes.map((n) => n.lane), [0, 0, 0]);
  assert.deepEqual(layout.nodes.map((n) => n.hasIncoming), [false, true, true]);
  assert.deepEqual(layout.edges, [
    { row: 0, fromLane: 0, toLane: 0 },
    { row: 1, fromLane: 0, toLane: 0 },
  ]);
});

test("a merge opens a second lane for the feature branch and closes it back", async () => {
  const { computeGitGraph } = await import("../src/gitGraph.ts");
  // Mirrors: main "A" -> "C" (main work), feature branch "A" -> "F2", then
  // merge "M" with parents [C, F2] — the same topology git log --topo-order
  // produces for a real `git merge --no-ff feature`.
  const commits = [
    { hash: "M", parents: ["C", "F2"], author: "a", date: 4, subject: "merge feature", refs: ["HEAD -> main"] },
    { hash: "F2", parents: ["A"], author: "a", date: 3, subject: "feature work", refs: ["feature"] },
    { hash: "C", parents: ["A"], author: "a", date: 2, subject: "main work", refs: [] },
    { hash: "A", parents: [], author: "a", date: 1, subject: "first", refs: [] },
  ];
  const layout = computeGitGraph(commits);
  const laneOf = (hash) => layout.nodes.find((n) => n.hash === hash).lane;
  assert.equal(laneOf("M"), 0);
  assert.equal(laneOf("C"), 0);
  // The feature commit gets its own lane, distinct from the mainline.
  assert.notEqual(laneOf("F2"), laneOf("M"));
  assert.equal(layout.laneCount, 2);
  // The merge commit fans out onto the feature lane.
  assert.ok(
    layout.edges.some(
      (e) => e.row === 0 && e.fromLane === laneOf("M") && e.toLane === laneOf("F2"),
    ),
  );
  // Both branches share "A" as an ancestor: the lines converge into one lane
  // by the time "A" is reached, rather than rendering two parallel tracks.
  const rootRow = layout.nodes.find((n) => n.hash === "A").row;
  const incomingAtRoot = layout.edges.filter((e) => e.row === rootRow - 1);
  const rootLanes = new Set(incomingAtRoot.map((e) => e.toLane));
  assert.equal(rootLanes.size, 1);
  assert.equal([...rootLanes][0], laneOf("A"));
});

test("a branch commit forks from its visible parent without a gap", async () => {
  const { computeGitGraph } = await import("../src/gitGraph.ts");
  const commits = [
    { hash: "C", parents: ["A"], author: "a", date: 3, subject: "main", refs: [] },
    { hash: "F", parents: ["A"], author: "a", date: 2, subject: "feature", refs: [] },
    { hash: "A", parents: [], author: "a", date: 1, subject: "root", refs: [] },
  ];
  const layout = computeGitGraph(commits);
  const featureLane = layout.nodes.find((node) => node.hash === "F").lane;
  assert.notEqual(featureLane, layout.nodes[0].lane);
  assert.ok(
    layout.edges.some(
      (edge) =>
        edge.row === 0 &&
        edge.fromLane === layout.nodes[0].lane &&
        edge.toLane === featureLane,
    ),
  );
});

test("a branch reconnects when its first visible parent is still below it", async () => {
  const { computeGitGraph } = await import("../src/gitGraph.ts");
  const layout = computeGitGraph([
    { hash: "main", parents: ["base"] },
    { hash: "feature-tip", parents: ["feature-parent"] },
    { hash: "feature-parent", parents: ["base"] },
    { hash: "base", parents: [] },
  ]);

  assert.ok(
    layout.edges.some(
      (edge) =>
        edge.row === 0 && edge.fromLane === 0 && edge.toLane === 1,
    ),
    "the feature line should fork from the main line even through an unseen parent",
  );
});

test("classifyRefs labels HEAD, local branches, remotes and tags", async () => {
  const { classifyRefs } = await import("../src/gitGraph.ts");
  const badges = classifyRefs(["HEAD -> main", "origin/main", "tag: v1.0"]);
  assert.deepEqual(badges, [
    { label: "main", kind: "head" },
    { label: "origin/main", kind: "remote" },
    { label: "v1.0", kind: "tag" },
  ]);
});

test("laneColor is deterministic and cycles through the palette", async () => {
  const { laneColor } = await import("../src/gitGraph.ts");
  assert.equal(laneColor(0), laneColor(0));
  assert.notEqual(laneColor(0), laneColor(1));
  // The palette repeats once lanes exceed its length.
  const paletteSize = new Set(Array.from({ length: 32 }, (_, i) => laneColor(i))).size;
  assert.ok(paletteSize < 32);
  assert.equal(laneColor(0), laneColor(paletteSize));
});
