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

test("nudging the only member of an auto-sized boundary keeps it", async () => {
  const { dropGroup } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  assert.equal(dropGroup(d, placeNodes(d), "a", centre(10, 6)), "g");
});

test("nudging one of two members keeps it in the boundary", async () => {
  const { dropGroup } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" }), node("b", 200, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  assert.equal(dropGroup(d, placeNodes(d), "b", centre(220, 0)), "g");
});

test("a drop outside every boundary clears the group", async () => {
  const { dropGroup } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" }), node("b", 200, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  assert.equal(dropGroup(d, placeNodes(d), "b", centre(900, 900)), "");
});

test("a drop into a smaller boundary moves the component", async () => {
  const { dropGroup } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 20, 40, { group: "big" })],
    [
      { id: "big", label: "Big", x: 0, y: 0, w: 800, h: 600 },
      { id: "small", label: "Small", x: 400, y: 100, w: 300, h: 200 },
    ],
  );
  assert.equal(dropGroup(d, placeNodes(d), "a", centre(450, 140)), "small");
  assert.equal(dropGroup(d, placeNodes(d), "a", centre(60, 60)), "big");
});

test("a component without a boundary joins the one under the drop", async () => {
  const { dropGroup } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 900, 900)],
    [{ id: "g", label: "G", x: 0, y: 0, w: 400, h: 300 }],
  );
  assert.equal(dropGroup(d, placeNodes(d), "a", centre(100, 100)), "g");
});

test("the last member leaving an unbounded boundary writes its bounds", async () => {
  const { leaveBounds } = await board;
  const { placeNodes, applyOps } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  const pos = placeNodes(d);
  const ops = leaveBounds(d, pos, "a", "");
  assert.equal(ops.length, 1);
  assert.equal(ops[0].op, "resize-group");
  const after = applyOps(d, [
    ...ops,
    { op: "set", id: "a", field: "group", value: "" },
  ]);
  const g = after.groups[0];
  assert.deepEqual([g.x, g.y, g.w, g.h], [-10, -30, 190, 100]);
  const rects = (await model).groupRects(after, placeNodes(after));
  assert.ok(rects.has("g"));
});

test("bounds are left alone when the boundary has bounds or other members", async () => {
  const { leaveBounds } = await board;
  const { placeNodes } = await model;
  const bounded = diagram(
    [node("a", 0, 0, { group: "g" })],
    [{ id: "g", label: "G", x: 0, y: 0, w: 300, h: 200 }],
  );
  assert.deepEqual(leaveBounds(bounded, placeNodes(bounded), "a", ""), []);
  const shared = diagram(
    [node("a", 0, 0, { group: "g" }), node("b", 200, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  assert.deepEqual(leaveBounds(shared, placeNodes(shared), "a", ""), []);
  assert.deepEqual(leaveBounds(shared, placeNodes(shared), "a", "g"), []);
});

test("dropRects leaves the dragged component out of a wrapping boundary", async () => {
  const { dropRects } = await board;
  const { placeNodes, groupRects } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" }), node("b", 600, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  const pos = placeNodes(d);
  assert.ok(groupRects(d, pos).get("g").w > 700);
  assert.equal(dropRects(d, pos, "b").get("g").w, 190);
});

test("groupAt picks the smallest boundary and none outside", async () => {
  const { groupAt } = await board;
  const rects = new Map([
    ["big", { x: 0, y: 0, w: 800, h: 600 }],
    ["small", { x: 100, y: 100, w: 200, h: 100 }],
  ]);
  assert.equal(groupAt(rects, { x: 150, y: 150 }), "small");
  assert.equal(groupAt(rects, { x: 500, y: 500 }), "big");
  assert.equal(groupAt(rects, { x: 900, y: 900 }), "");
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
  const { placeNodes } = await model;
  const d = diagram(
    [
      node("a", 0, 0, { group: "g", shape: "circle" }),
      node("b", 0, 300, { shape: "person" }),
    ],
    [{ id: "g", label: "G", x: 0, y: 0, w: 100, h: 100 }],
  );
  const pos = placeNodes(d);
  // a circle dropped so its centre (36, 36) is inside the boundary
  assert.equal(dropGroup(d, pos, "a", { x: 0, y: 0 }), "g");
  // a person (120x84) dropped at x 90 has its centre at x 150, outside
  assert.equal(dropGroup(d, pos, "b", { x: 90, y: 0 }), "");
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

test("deleting a selection removes links first and keeps a boundary that lost every member", async () => {
  const { deleteOps } = await board;
  const { placeNodes } = await model;
  const d = diagram(
    [node("a", 0, 0, { group: "g" }), node("b", 300, 0, { group: "g" })],
    [{ id: "g", label: "G" }],
  );
  d.edges = [{ id: "ab", from: "a", to: "b" }];
  d.notes = [{ id: "n", text: "?", x: 0, y: 200 }];
  const ops = deleteOps(d, placeNodes(d), [
    { type: "node", id: "a" },
    { type: "node", id: "b" },
    { type: "note", id: "n" },
    { type: "edge", id: "ab" },
  ]);
  assert.equal(ops[0].op, "resize-group");
  assert.equal(ops[0].id, "g");
  assert.deepEqual(
    ops.slice(1).map((o) => o.id),
    ["ab", "a", "b", "n"],
  );
  const some = deleteOps(d, placeNodes(d), [{ type: "node", id: "a" }]);
  assert.deepEqual(some, [{ op: "delete", id: "a" }]);
  const withGroup = deleteOps(d, placeNodes(d), [
    { type: "node", id: "a" },
    { type: "node", id: "b" },
    { type: "group", id: "g" },
  ]);
  assert.deepEqual(
    withGroup.map((o) => o.op),
    ["delete", "delete", "delete"],
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
