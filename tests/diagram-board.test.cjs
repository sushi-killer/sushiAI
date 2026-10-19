const { test } = require("node:test");
const assert = require("node:assert/strict");

const dir = "../src/extensions/preview/diagram/";
const board = import(dir + "boardMath.ts");
const model = import(dir + "model.ts");

const node = (id, x, y, extra = {}) => ({
  id,
  kind: "app",
  title: id,
  x,
  y,
  ...extra,
});
const diagram = (nodes, groups) => ({
  version: 1,
  groups,
  nodes,
  edges: [],
  notes: [],
});
const centre = (x, y) => ({ x, y });

const box = (id, x, y, w, h) => ({ id, label: id, x, y, w, h });

test("a nudge inside the boundary keeps the group", async () => {
  const { dropGroup } = await board;
  const d = diagram(
    [node("a", 0, 0, { group: "g" })],
    [box("g", 0, 0, 300, 200)],
  );
  assert.equal(dropGroup(d, "a", centre(10, 6)), "g");
});

test("a drop outside every boundary clears the group", async () => {
  const { dropGroup } = await board;
  const d = diagram(
    [node("a", 0, 0, { group: "g" })],
    [box("g", 0, 0, 300, 200)],
  );
  assert.equal(dropGroup(d, "a", centre(900, 900)), "");
});

test("a drop into a smaller boundary moves the component", async () => {
  const { dropGroup } = await board;
  const d = diagram(
    [node("a", 20, 40, { group: "big" })],
    [box("big", 0, 0, 800, 600), box("small", 400, 100, 300, 200)],
  );
  assert.equal(dropGroup(d, "a", centre(450, 140)), "small");
  assert.equal(dropGroup(d, "a", centre(60, 60)), "big");
});

test("a component without a boundary joins the one under the drop", async () => {
  const { dropGroup } = await board;
  const d = diagram([node("a", 900, 900)], [box("g", 0, 0, 400, 300)]);
  assert.equal(dropGroup(d, "a", centre(100, 100)), "g");
});

test("freeSpot lands below everything and stays inside the file range", async () => {
  const { freeSpot, clampSpot } = await board;
  const d = diagram(
    [node("a", 100, 40)],
    [{ id: "g", label: "G", x: 80, y: 0, w: 300, h: 400 }],
  );
  const at = freeSpot(d, 170, 62);
  assert.ok(at.y >= 440);
  assert.equal(at.x % 2, 0);
  assert.deepEqual(freeSpot(diagram([], []), 170, 62), { x: 0, y: 0 });
  const far = diagram([node("a", 0, 99900)], []);
  assert.equal(freeSpot(far, 170, 62).y, 100000 - 62);
  assert.deepEqual(clampSpot({ x: -999999, y: 999999 }, 420, 240), {
    x: -100000,
    y: 100000 - 240,
  });
});

test("drop and free-spot math uses each block's own size", async () => {
  const { dropGroup, freeSpot } = await board;
  const d = diagram(
    [
      node("a", 0, 0, { group: "g", shape: "circle" }),
      node("b", 0, 300, { shape: "person" }),
    ],
    [{ id: "g", label: "G", x: 0, y: 0, w: 100, h: 100 }],
  );
  // a circle dropped so its centre (36, 36) is inside the boundary
  assert.equal(dropGroup(d, "a", { x: 0, y: 0 }), "g");
  // a person (120x84) dropped at x 90 has its centre at x 150, outside
  assert.equal(dropGroup(d, "b", { x: 90, y: 0 }), "");
  // below the person: 300 + 84 + gap 40
  assert.equal(freeSpot(d, 10, 10).y, 424);
});

const world = () =>
  diagram(
    [
      node("a", 0, 100, { group: "g" }),
      node("b", 400, 100, { group: "g" }),
      node("c", 1000, 100),
    ],
    [{ id: "g", label: "G", x: -20, y: 40, w: 700, h: 300 }],
  );

test("a marquee picks what it touches, a group by its header, and links between picked blocks", async () => {
  const { marqueeHits } = await board;
  const { placeNodes } = await model;
  const d = {
    ...world(),
    notes: [{ id: "n", text: "?", x: 1000, y: 400 }],
    edges: [
      { id: "ab", from: "a", to: "b" },
      { id: "bc", from: "b", to: "c" },
    ],
  };
  const key = (hits) => hits.map((h) => `${h.type}:${h.id}`).sort();
  const pos = placeNodes(d);
  assert.deepEqual(key(marqueeHits(d, pos, { x: 10, y: 110, w: 500, h: 20 })), [
    "edge:ab",
    "node:a",
    "node:b",
  ]);
  assert.deepEqual(key(marqueeHits(d, pos, { x: 0, y: 200, w: 5, h: 5 })), []);
  assert.deepEqual(key(marqueeHits(d, pos, { x: 0, y: 45, w: 5, h: 5 })), [
    "group:g",
  ]);
  assert.deepEqual(
    key(marqueeHits(d, pos, { x: 990, y: 90, w: 400, h: 400 })),
    ["node:c", "note:n"],
  );
});

test("a node moves with its group when both are picked, not by itself", async () => {
  const { moveSet, moveOps } = await board;
  const { placeNodes } = await model;
  const d = world();
  const picks = [
    { type: "group", id: "g" },
    { type: "node", id: "a" },
    { type: "node", id: "c" },
  ];
  const set = moveSet(d, picks);
  assert.deepEqual(set, { nodes: ["c"], notes: [], groups: ["g"] });
  const ops = moveOps(d, placeNodes(d), set, 10, -4);
  assert.deepEqual(ops, [
    {
      op: "move-group",
      id: "g",
      rect: { x: -10, y: 36, w: 700, h: 300 },
      dx: 10,
      dy: -4,
    },
    { op: "move", id: "c", x: 1010, y: 96 },
  ]);
  assert.deepEqual(moveOps(d, placeNodes(d), set, 0, 0), []);
});

test("moving a note and a node writes one move op each", async () => {
  const { moveSet, moveOps } = await board;
  const { placeNodes } = await model;
  const d = { ...world(), notes: [{ id: "n", text: "?", x: 5, y: 6 }] };
  const set = moveSet(d, [
    { type: "note", id: "n" },
    { type: "node", id: "c" },
    { type: "edge", id: "x" },
  ]);
  assert.deepEqual(moveOps(d, placeNodes(d), set, 2, 2), [
    { op: "move", id: "c", x: 1002, y: 102 },
    { op: "move", id: "n", x: 7, y: 8 },
  ]);
});

test("deleting a selection removes links first, then blocks, notes and groups", async () => {
  const { deleteOps } = await board;
  const ops = deleteOps([
    { type: "group", id: "g" },
    { type: "node", id: "a" },
    { type: "note", id: "n" },
    { type: "edge", id: "ab" },
  ]);
  assert.deepEqual(
    ops.map((o) => [o.op, o.id]),
    [
      ["delete", "ab"],
      ["delete", "a"],
      ["delete", "n"],
      ["delete", "g"],
    ],
  );
});

test("a selection loses ids the file no longer has", async () => {
  const { livePicks, allPicks } = await board;
  const d = world();
  const all = allPicks(d);
  assert.equal(all.length, 4);
  const kept = livePicks({ ...d, nodes: d.nodes.slice(0, 1) }, all);
  assert.deepEqual(
    kept.map((p) => `${p.type}:${p.id}`),
    ["group:g", "node:a"],
  );
});

test("countLabel is singular for one", async () => {
  const { countLabel } = await board;
  assert.equal(countLabel(1, 1), "1 block · 1 link");
  assert.equal(countLabel(0, 2), "0 blocks · 2 links");
  assert.equal(countLabel(9, 10), "9 blocks · 10 links");
});

test("the marquee and the free spot use the wrapped size of a block and a note", async () => {
  const { marqueeHits, freeSpot } = await board;
  const { nodeSize, noteSize } = await model;
  const long = node("a", 0, 0, {
    title:
      "A very long title for one block that has to wrap onto many lines. ".repeat(
        3,
      ),
  });
  const d = {
    ...diagram([long], []),
    notes: [{ id: "n", text: "1\n2\n3\n4", x: 400, y: 0 }],
  };
  const pos = new Map([["a", { x: 0, y: 0 }]]);
  const low = nodeSize(long).h - 2;
  assert.ok(low > 62);
  const hits = marqueeHits(d, pos, { x: 10, y: low, w: 4, h: 1 });
  assert.deepEqual(hits, [{ type: "node", id: "a" }]);
  const note = noteSize(d.notes[0]);
  assert.ok(note.h > 30);
  assert.deepEqual(marqueeHits(d, pos, { x: 410, y: note.h - 2, w: 4, h: 1 }), [
    { type: "note", id: "n" },
  ]);
  const spot = freeSpot(d, 10, 10);
  assert.ok(spot.y >= Math.max(nodeSize(long).h, note.h));
});
