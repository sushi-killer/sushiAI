const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/extensions/preview/diagram/model.ts");
const measure = import("../src/extensions/preview/diagram/measure.ts");
const board = import("../src/extensions/preview/diagram/boardMath.ts");

const base = () => ({
  version: 1,
  groups: [{ id: "cloud", label: "Cloud", x: 0, y: 0, w: 400, h: 300 }],
  nodes: [
    { id: "api", kind: "service", title: "API", group: "cloud" },
    { id: "db", kind: "data", title: "DB", tech: "pg", delta: "new" },
  ],
  edges: [{ id: "e1", from: "api", to: "db", label: "sql" }],
  notes: [{ id: "n1", text: "why?", x: 5, y: 6 }],
});

const textNode = (id, extra = {}) => ({
  id,
  type: "text",
  text: id,
  x: 0,
  y: 0,
  width: 170,
  height: 62,
  ...extra,
});
const file = (nodes = [], edges = [], more = {}) =>
  JSON.stringify({ nodes, edges, ...more });
const canvasBase = () => ({
  nodes: [
    {
      id: "cloud",
      type: "group",
      label: "Cloud",
      x: 0,
      y: 0,
      width: 400,
      height: 300,
    },
    textNode("api", {
      text: "API",
      x: 20,
      y: 40,
      sushiai: { kind: "service" },
    }),
    textNode("db", {
      text: "DB\npg",
      x: 500,
      y: 40,
      height: 66,
      color: "4",
      sushiai: { delta: "new" },
    }),
    textNode("n1", {
      text: "why?",
      x: 5,
      y: 6,
      width: 70,
      height: 32,
      color: "3",
      sushiai: { note: true },
    }),
  ],
  edges: [{ id: "e1", fromNode: "api", toNode: "db", label: "sql" }],
});

test("parseDiagram reads JSON Canvas and defaults missing arrays", async () => {
  const { parseDiagram } = await model;
  const r = parseDiagram("{}");
  assert.deepEqual(r.diagram, {
    version: 1,
    groups: [],
    nodes: [],
    edges: [],
    notes: [],
  });
  const { diagram } = parseDiagram(JSON.stringify(canvasBase()));
  assert.deepEqual(diagram.groups, [
    { id: "cloud", label: "Cloud", x: 0, y: 0, w: 400, h: 300 },
  ]);
  assert.deepEqual(diagram.nodes, [
    { id: "api", title: "API", kind: "service", x: 20, y: 40, group: "cloud" },
    { id: "db", title: "DB", tech: "pg", delta: "new", x: 500, y: 40 },
  ]);
  assert.deepEqual(diagram.edges, [
    { id: "e1", from: "api", to: "db", label: "sql" },
  ]);
  assert.deepEqual(diagram.notes, [{ id: "n1", text: "why?", x: 5, y: 6 }]);
});

test("parseDiagram rejects bad files with an error naming the item", async () => {
  const { parseDiagram } = await model;
  const bad = (mut) => {
    const d = canvasBase();
    mut(d);
    return parseDiagram(JSON.stringify(d)).error;
  };
  assert.match(parseDiagram("{").error, /not valid JSON/);
  assert.match(parseDiagram("[]").error, /object/);
  assert.match(parseDiagram('{"sushiai":{"version":2}}').error, /version/);
  assert.match(parseDiagram('{"nodes":{}}').error, /nodes/);
  assert.match(parseDiagram('{"edges":{}}').error, /edges/);
  assert.equal(
    bad((d) => (d.nodes[2].sushiai.kind = "db")),
    'nodes[2] (db): unknown kind "db"',
  );
  assert.match(
    bad((d) => (d.nodes[2].id = "api")),
    /nodes\[2\] \(api\): duplicate id/,
  );
  assert.match(
    bad((d) => (d.nodes[0].id = "e1")),
    /duplicate id/,
  );
  assert.match(
    bad((d) => (d.nodes[1].id = "")),
    /id must be a non-empty/,
  );
  assert.match(
    bad((d) => delete d.nodes[1].type),
    /type must be a string/,
  );
  assert.match(
    bad((d) => (d.edges[0].toNode = "ghost")),
    /edges\[0\] \(e1\).*ghost/,
  );
  assert.match(
    bad((d) => (d.nodes[2].sushiai.delta = "old")),
    /unknown delta/,
  );
  assert.match(
    bad((d) => (d.nodes[1].sushiai.status = "done")),
    /unknown status/,
  );
  assert.match(
    bad((d) => (d.edges[0].sushiai = { style: "wavy" })),
    /unknown style/,
  );
  assert.match(
    bad((d) => (d.edges[0].toEnd = "dot")),
    /unknown toEnd/,
  );
  assert.match(
    bad((d) => (d.edges[0].fromSide = "middle")),
    /unknown fromSide/,
  );
  assert.match(
    bad((d) => (d.nodes[1].x = "1")),
    /x must be a finite/,
  );
  assert.match(
    bad((d) => delete d.nodes[3].y),
    /nodes\[3\] \(n1\)/,
  );
  assert.match(
    bad((d) => delete d.nodes[1].text),
    /text must be a string/,
  );
  assert.match(
    bad((d) => (d.nodes[1].sushiai = [])),
    /sushiai must be an object/,
  );
});

test("unknown fields survive parse and serialize", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  const d = canvasBase();
  d.future = { a: 1 };
  d.sushiai = { version: 1, title: "T", later: true };
  d.nodes[1].owner = "team";
  d.nodes[1].color = "#336699";
  d.nodes[1].sushiai.shapeHint = 2;
  d.nodes[0].background = "bg.png";
  d.edges[0].weight = 3;
  d.edges[0].fromSide = "right";
  d.edges[0].color = "5";
  d.nodes.push({ id: "x", type: "icon", x: 1, y: 2, width: 3, height: 4 });
  d.edges.push({ id: "ex", fromNode: "cloud", toNode: "api" });
  const r = parseDiagram(JSON.stringify(d));
  const out = JSON.parse(serializeDiagram(r.diagram));
  const find = (list, id) => list.find((i) => i.id === id);
  assert.deepEqual(out.future, { a: 1 });
  assert.deepEqual(out.sushiai, { version: 1, title: "T", later: true });
  assert.equal(find(out.nodes, "api").owner, "team");
  assert.equal(find(out.nodes, "api").color, "#336699");
  assert.equal(find(out.nodes, "api").sushiai.shapeHint, 2);
  assert.equal(find(out.nodes, "cloud").background, "bg.png");
  assert.equal(find(out.edges, "e1").weight, 3);
  assert.equal(find(out.edges, "e1").fromSide, "right");
  assert.equal(find(out.edges, "e1").color, "5");
  assert.deepEqual(find(out.nodes, "x"), d.nodes[4]);
  assert.deepEqual(find(out.edges, "ex"), d.edges[1]);
  // a link to a node that was deleted does not dangle
  const gone = serializeDiagram(applyDeleted(r.diagram, "api", await model));
  assert.equal(find(JSON.parse(gone).edges, "ex"), undefined);
});

function applyDeleted(d, id, m) {
  return m.applyOp(d, { op: "delete", id });
}

test("node order and top-level order of edges are kept", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  const d = canvasBase();
  d.nodes.reverse();
  const out = JSON.parse(
    serializeDiagram(parseDiagram(JSON.stringify(d)).diagram),
  );
  assert.deepEqual(
    out.nodes.map((n) => n.id),
    ["n1", "db", "api", "cloud"],
  );
});

test("add-node, connect: no-ops and happy paths", async () => {
  const { applyOp } = await model;
  const d = base();
  const added = applyOp(d, {
    op: "add-node",
    node: { id: "q", kind: "app", title: "Q" },
  });
  assert.equal(added.nodes.length, 3);
  assert.equal(d.nodes.length, 2);
  assert.equal(
    applyOp(d, {
      op: "add-node",
      node: { id: "api", kind: "app", title: "X" },
    }),
    d,
  );
  const link = (e) => applyOp(d, { op: "connect", edge: e });
  assert.equal(link({ id: "e2", from: "db", to: "api" }).edges.length, 2);
  assert.equal(link({ id: "e2", from: "ghost", to: "api" }), d);
  assert.equal(link({ id: "e2", from: "api", to: "ghost" }), d);
  assert.equal(link({ id: "e2", from: "api", to: "api" }), d);
  assert.equal(link({ id: "e1", from: "db", to: "api" }), d);
  assert.equal(link({ id: "e2", from: "api", to: "db" }), d);
});

test("move works for nodes and notes, missing id is a no-op", async () => {
  const { applyOp } = await model;
  const d = base();
  const a = applyOp(d, { op: "move", id: "api", x: 1, y: 2 });
  assert.deepEqual([a.nodes[0].x, a.nodes[0].y], [1, 2]);
  const b = applyOp(d, { op: "move", id: "n1", x: 7, y: 8 });
  assert.deepEqual([b.notes[0].x, b.notes[0].y], [7, 8]);
  assert.equal(applyOp(d, { op: "move", id: "zzz", x: 1, y: 2 }), d);
  assert.equal(applyOp(d, { op: "move", id: "api", x: NaN, y: 2 }), d);
});

test("set edits fields, empty removes optional ones, invalid is a no-op", async () => {
  const { applyOp } = await model;
  const d = base();
  const set = (id, field, value) => applyOp(d, { op: "set", id, field, value });
  assert.equal(set("api", "title", "Gateway").nodes[0].title, "Gateway");
  assert.equal(set("api", "title", ""), d);
  assert.equal(set("api", "kind", "external"), d);
  assert.equal(set("db", "delta", "changed").nodes[1].delta, "changed");
  assert.equal("delta" in set("db", "delta", "").nodes[1], false);
  assert.equal(set("db", "delta", "bogus"), d);
  assert.equal("tech" in set("db", "tech", "").nodes[1], false);
  assert.equal(set("db", "tech", "sqlite").nodes[1].tech, "sqlite");
  assert.equal("group" in set("api", "group", "").nodes[0], false);
  assert.equal(set("db", "group", "cloud").nodes[1].group, "cloud");
  assert.equal(set("db", "group", "nope"), d);
  assert.equal(set("e1", "label", "grpc").edges[0].label, "grpc");
  assert.equal("label" in set("e1", "label", "").edges[0], false);
  assert.equal(set("e1", "style", "guess").edges[0].style, "guess");
  assert.equal(set("e1", "style", "wavy"), d);
  assert.equal(set("cloud", "label", "Cloud").groups[0].label, "Cloud");
  assert.equal(set("ghost", "title", "x"), d);
  assert.equal(set("e1", "title", "x"), d);
});

test("delete removes node with its edges, edge, note, group", async () => {
  const { applyOp } = await model;
  const d = base();
  const n = applyOp(d, { op: "delete", id: "db" });
  assert.deepEqual(
    n.nodes.map((x) => x.id),
    ["api"],
  );
  assert.equal(n.edges.length, 0);
  assert.equal(n.groups.length, 1);
  assert.equal(applyOp(d, { op: "delete", id: "e1" }).edges.length, 0);
  assert.equal(applyOp(d, { op: "delete", id: "n1" }).notes.length, 0);
  const g = applyOp(d, { op: "delete", id: "cloud" });
  assert.equal(g.groups.length, 0);
  assert.equal("group" in g.nodes[0], false);
  assert.equal(g.nodes.length, 2);
  assert.equal(applyOp(d, { op: "delete", id: "ghost" }), d);
});

test("accept removes status, reject deletes", async () => {
  const { applyOp } = await model;
  const d = base();
  d.nodes[1].status = "proposed";
  d.edges[0].status = "proposed";
  const a = applyOp(d, { op: "accept", id: "db" });
  assert.equal("status" in a.nodes[1], false);
  assert.equal(a.edges[0].status, "proposed");
  assert.equal(
    "status" in applyOp(d, { op: "accept", id: "e1" }).edges[0],
    false,
  );
  const r = applyOp(d, { op: "reject", id: "db" });
  assert.equal(r.nodes.length, 1);
  assert.equal(r.edges.length, 0);
  assert.equal(applyOp(d, { op: "accept", id: "ghost" }).nodes.length, 2);
});

test("newId slugs and keeps ids unique", async () => {
  const { newId } = await model;
  const d = base();
  assert.equal(newId(d, "Cloud API!"), "cloud-api");
  assert.equal(newId(d, "API"), "api-2");
  d.nodes.push({ id: "api-2", kind: "app", title: "x" });
  assert.equal(newId(d, "api"), "api-3");
  assert.equal(newId(d, "cloud"), "cloud-2");
  assert.equal(newId(d, "e1"), "e1-2");
  assert.equal(newId(d, "???"), "item");
});

const bare = (n) => ({
  version: 1,
  groups: [],
  nodes: n,
  edges: [],
  notes: [],
});
const nd = (id, extra = {}) => ({ id, kind: "app", title: id, ...extra });
const rectOf = (p) => ({ x: p.x, y: p.y, w: 170, h: 62 });
function assertNoOverlap(pos) {
  const all = [...pos.values()].map(rectOf);
  for (let i = 0; i < all.length; i++) {
    for (let j = i + 1; j < all.length; j++) {
      const a = all[i];
      const b = all[j];
      const hit =
        a.x < b.x + b.w + 20 &&
        b.x < a.x + a.w + 20 &&
        a.y < b.y + b.h + 20 &&
        b.y < a.y + a.h + 20;
      assert.equal(hit, false, `${i} overlaps ${j}`);
    }
  }
}

test("placeNodes keeps explicit, lays a lattice, is deterministic", async () => {
  const { placeNodes } = await model;
  const d = bare([
    nd("a", { x: 26, y: 46 }),
    nd("b"),
    nd("c"),
    nd("d"),
    nd("e"),
  ]);
  const pos = placeNodes(d);
  assert.deepEqual(pos.get("a"), { x: 26, y: 46 });
  assert.deepEqual(pos.get("b"), { x: 236, y: 46 });
  assert.deepEqual(pos.get("c"), { x: 446, y: 46 });
  assert.deepEqual(pos.get("d"), { x: 26, y: 136 });
  assertNoOverlap(pos);
  assert.deepEqual([...placeNodes(d)], [...pos]);
});

test("adding a node never moves existing ones", async () => {
  const { placeNodes, materialize, applyOp } = await model;
  let d = bare([nd("a"), nd("b"), nd("c")]);
  d = materialize(d, placeNodes(d));
  const before = placeNodes(d);
  for (const id of ["d", "e", "f", "g"]) {
    d = applyOp(d, { op: "add-node", node: nd(id) });
    const after = placeNodes(d);
    for (const [k, p] of before) assert.deepEqual(after.get(k), p);
    assertNoOverlap(after);
    d = materialize(d, after);
  }
  // not materialised: appending still keeps the earlier slots
  const loose = bare([nd("a"), nd("b")]);
  const first = placeNodes(loose);
  const more = placeNodes(bare([...loose.nodes, nd("c")]));
  for (const [k, p] of first) assert.deepEqual(more.get(k), p);
});

test("placeNodes puts group members in the group's bounds", async () => {
  const { placeNodes } = await model;
  const d = {
    version: 1,
    groups: [{ id: "g", label: "G", x: 500, y: 300, w: 480, h: 200 }],
    nodes: [
      nd("a", { group: "g" }),
      nd("b", { group: "g" }),
      nd("c", { group: "g" }),
    ],
    edges: [],
    notes: [],
  };
  const pos = placeNodes(d);
  assert.deepEqual(pos.get("a"), { x: 520, y: 330 });
  assert.deepEqual(pos.get("b"), { x: 730, y: 330 });
  assert.deepEqual(pos.get("c"), { x: 520, y: 420 });
  assertNoOverlap(pos);
});

test("lattice skips group rects", async () => {
  const { placeNodes } = await model;
  const d = {
    version: 1,
    groups: [{ id: "g", label: "G", x: 0, y: 0, w: 300, h: 100 }],
    nodes: [nd("a")],
    edges: [],
    notes: [],
  };
  const p = placeNodes(d).get("a");
  assert.equal(p.x >= 300 || p.y >= 100, true);
});

test("groupRects gives each group its own bounds and nothing grows them", async () => {
  const { groupRects } = await model;
  const d = {
    version: 1,
    groups: [
      { id: "g", label: "G", x: 10, y: 20, w: 100, h: 50 },
      { id: "e", label: "E" },
    ],
    nodes: [nd("a", { group: "g", x: 500, y: 500 })],
    edges: [],
    notes: [],
  };
  const r = groupRects(d);
  assert.deepEqual(r.get("g"), { x: 10, y: 20, w: 100, h: 50 });
  assert.equal(r.has("e"), false);
});

test("groupAt picks the smallest group and none outside", async () => {
  const { groupAt } = await model;
  const rects = new Map([
    ["big", { x: 0, y: 0, w: 800, h: 600 }],
    ["small", { x: 100, y: 100, w: 200, h: 100 }],
  ]);
  assert.equal(groupAt(rects, { x: 150, y: 150 }), "small");
  assert.equal(groupAt(rects, { x: 500, y: 500 }), "big");
  assert.equal(groupAt(rects, { x: 900, y: 900 }), "");
});

test("materialize writes positions only where missing", async () => {
  const { materialize } = await model;
  const d = bare([nd("a", { x: 1, y: 2 }), nd("b")]);
  const m = materialize(
    d,
    new Map([
      ["a", { x: 9, y: 9 }],
      ["b", { x: 3, y: 4 }],
    ]),
  );
  assert.deepEqual([m.nodes[0].x, m.nodes[0].y], [1, 2]);
  assert.deepEqual([m.nodes[1].x, m.nodes[1].y], [3, 4]);
  assert.equal(d.nodes[1].x, undefined);
});

test("routeEdge: vertical, horizontal, elbow", async () => {
  const { routeEdge } = await model;
  const r = (x, y) => ({ x, y, w: 100, h: 50 });
  // same column: a's centre x (50) is inside b
  assert.deepEqual(routeEdge(r(0, 0), r(20, 200)), [
    { x: 50, y: 50 },
    { x: 50, y: 200 },
  ]);
  // b above a
  assert.deepEqual(routeEdge(r(0, 200), r(0, 0)), [
    { x: 50, y: 200 },
    { x: 50, y: 50 },
  ]);
  // a's centre outside b, b's centre inside a
  assert.deepEqual(routeEdge(r(0, 0), r(80, 200))[0].x, 90);
  // x ranges overlap only in a sliver: overlap middle
  assert.deepEqual(routeEdge(r(0, 0), r(95, 200)), [
    { x: 97.5, y: 50 },
    { x: 97.5, y: 200 },
  ]);
  // horizontal
  assert.deepEqual(routeEdge(r(0, 0), r(300, 10)), [
    { x: 100, y: 25 },
    { x: 300, y: 25 },
  ]);
  assert.deepEqual(routeEdge(r(300, 0), r(0, 0)), [
    { x: 300, y: 25 },
    { x: 100, y: 25 },
  ]);
  // elbow, wider gap on x: leave sideways, enter b from the top
  assert.deepEqual(routeEdge(r(0, 0), r(400, 100)), [
    { x: 100, y: 25 },
    { x: 450, y: 25 },
    { x: 450, y: 100 },
  ]);
  // elbow, wider gap on y: leave downward, enter b from its left
  assert.deepEqual(routeEdge(r(0, 0), r(150, 400)), [
    { x: 50, y: 50 },
    { x: 50, y: 425 },
    { x: 150, y: 425 },
  ]);
});

test("labelAt picks the middle of the longest segment", async () => {
  const { labelAt } = await model;
  assert.deepEqual(
    labelAt([
      { x: 0, y: 0 },
      { x: 50, y: 0 },
      { x: 50, y: 200 },
    ]),
    { x: 50, y: 100, vertical: true },
  );
  assert.deepEqual(
    labelAt([
      { x: 0, y: 0 },
      { x: 100, y: 0 },
    ]),
    { x: 50, y: 0, vertical: false },
  );
});

test("replay: owner ops applied to a file the agent changed keep both", async () => {
  const { applyOps, parseDiagram, serializeDiagram } = await model;
  const old = base();
  const ops = [
    { op: "move", id: "api", x: 40, y: 60 },
    { op: "set", id: "db", field: "tech", value: "sqlite" },
    { op: "add-node", node: nd("cache") },
    { op: "connect", edge: { id: "e9", from: "api", to: "cache" } },
  ];
  const newer = base();
  newer.nodes.push(nd("queue"));
  newer.edges.push({ id: "e8", from: "queue", to: "api" });
  const disk = parseDiagram(serializeDiagram(newer)).diagram;
  const out = applyOps(disk, ops);
  assert.deepEqual(
    out.nodes.map((n) => n.id),
    ["api", "db", "queue", "cache"],
  );
  assert.deepEqual(
    out.edges.map((e) => e.id),
    ["e1", "e8", "e9"],
  );
  assert.deepEqual([out.nodes[0].x, out.nodes[0].y], [40, 60]);
  assert.equal(out.nodes[1].tech, "sqlite");
  // the same ops against the old file give the old file plus the edits
  assert.equal(applyOps(old, ops).nodes.length, 3);
  // an op for an item the agent deleted is a no-op
  const gone = applyOps(bare([nd("a")]), [
    { op: "move", id: "api", x: 1, y: 1 },
  ]);
  assert.equal(gone.nodes[0].x, undefined);
});

test("parseDiagram rejects more than 1000 items", async () => {
  const { parseDiagram } = await model;
  const nodes = Array.from({ length: 1001 }, (_, i) => textNode(`n${i}`));
  assert.match(parseDiagram(file(nodes)).error, /1000/);
  assert.ok(parseDiagram(file(nodes.slice(1))).diagram);
  const edge = (i) => ({ id: `e${i}`, fromNode: "n1", toNode: "n2" });
  const mixed = nodes.slice(0, 600);
  const links = Array.from({ length: 401 }, (_, i) => edge(i));
  assert.match(parseDiagram(file(mixed, links)).error, /1000/);
});

test("1000 unplaced nodes place fast without overlap", async () => {
  const { placeNodes } = await model;
  const d = bare(Array.from({ length: 1000 }, (_, i) => nd(`n${i}`)));
  const t = Date.now();
  const pos = placeNodes(d);
  assert.ok(Date.now() - t < 500, `took ${Date.now() - t} ms`);
  assert.equal(pos.size, 1000);
  assertNoOverlap(pos);
});

test("materializeAfter pins B where the owner saw it after A was dragged", async () => {
  const { materializeAfter, placeNodes, applyOps } = await model;
  const fresh = bare([nd("a"), nd("b")]);
  const s0 = placeNodes(fresh).get("a");
  const ops = [{ op: "move", id: "a", x: s0.x + 210, y: s0.y }];
  const display = placeNodes(applyOps(fresh, ops));
  const saved = materializeAfter(fresh, ops);
  const a = saved.nodes[0];
  const b = saved.nodes[1];
  assert.deepEqual({ x: b.x, y: b.y }, display.get("b"));
  assert.deepEqual({ x: a.x, y: a.y }, { x: s0.x + 210, y: s0.y });
  assertNoOverlap(placeNodes(saved));
});

test("reidOps renames a clashing add-node and remaps later ops", async () => {
  const { reidOps, applyOps } = await model;
  const fresh = bare([nd("component", { title: "Agent's" })]);
  const ops = [
    { op: "add-node", node: nd("component", { title: "Mine" }) },
    { op: "set", id: "component", field: "title", value: "Owner's" },
    { op: "move", id: "component", x: 5, y: 6 },
    { op: "connect", edge: { id: "e", from: "component", to: "component-2" } },
  ];
  const re = reidOps(fresh, ops);
  assert.equal(re[0].node.id, "component-2");
  assert.equal(re[1].id, "component-2");
  assert.equal(re[2].id, "component-2");
  assert.equal(re[3].edge.from, "component-2");
  const out = applyOps(fresh, re);
  assert.deepEqual(out.nodes[0], nd("component", { title: "Agent's" }));
  assert.equal(out.nodes[1].title, "Owner's");
  assert.deepEqual([out.nodes[1].x, out.nodes[1].y], [5, 6]);
  assert.deepEqual(reidOps(fresh, [{ op: "move", id: "zz", x: 1, y: 1 }]), [
    { op: "move", id: "zz", x: 1, y: 1 },
  ]);
});

test("the shop fixture parses and its groups keep their bounds", async () => {
  const { parseDiagram, groupRects } = await model;
  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures/shop.canvas"),
    "utf8",
  );
  const { diagram } = parseDiagram(text);
  assert.equal(diagram.nodes.length, 9);
  assert.equal(diagram.edges.length, 8);
  assert.equal(diagram.groups.length, 4);
  assert.equal(diagram.notes.length, 1);
  const rects = groupRects(diagram);
  for (const g of diagram.groups) {
    assert.deepEqual(rects.get(g.id), { x: g.x, y: g.y, w: g.w, h: g.h });
  }
  assert.deepEqual(rects.get("phone"), { x: 16, y: 150, w: 190, h: 100 });
});

test("routeEdge dodges avoid rects and groupLabelRect sizes the label", async () => {
  const { routeEdge, groupLabelRect, parseDiagram, placeNodes, groupRects } =
    await model;
  const a = { x: 0, y: 0, w: 100, h: 50 };
  const b = { x: 0, y: 200, w: 100, h: 50 };
  const block = { x: 40, y: 100, w: 30, h: 20 };
  const seg = routeEdge(a, b, [block]);
  assert.equal(seg[0].x, 34);
  assert.equal(seg[0].x, seg[1].x);
  assert.deepEqual(
    routeEdge(a, b, [{ x: 0, y: 100, w: 100, h: 20 }]),
    routeEdge(a, b),
  );
  assert.deepEqual(
    routeEdge(a, b, [{ x: 0, y: 300, w: 100, h: 20 }]),
    routeEdge(a, b),
  );
  assert.deepEqual(groupLabelRect({ x: 10, y: 20, w: 99, h: 99 }, "abc"), {
    x: 20,
    y: 24,
    w: 16 + 3 * 6.6,
    h: 18,
  });

  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures/shop.canvas"),
    "utf8",
  );
  const { diagram } = parseDiagram(text);
  const pos = placeNodes(diagram);
  const rect = (id) => ({ ...pos.get(id), w: 170, h: 62 });
  const labels = diagram.groups.map((g) =>
    groupLabelRect(groupRects(diagram).get(g.id), g.label),
  );
  const [p, q] = routeEdge(rect("ledger"), rect("sync"), labels);
  const lab = labels[3];
  assert.equal(p.x, q.x);
  assert.equal(p.x > lab.x + lab.w || p.x < lab.x, true);
  for (const id of ["ledger", "sync"]) {
    assert.equal(p.x >= pos.get(id).x && p.x <= pos.get(id).x + 170, true);
  }
});

test("coordinates out of range are refused and cannot stall placement", async () => {
  const { parseDiagram, placeNodes } = await model;
  const doc = (n) => file([n]);
  assert.match(
    parseDiagram(doc(textNode("a", { x: 1e20, y: 0 }))).error,
    /nodes\[0\] \(a\)/,
  );
  assert.ok(parseDiagram(doc(textNode("a", { x: 1e5, y: -1e5 }))).diagram);
  const g = (extra) => file([{ id: "g", type: "group", label: "G", ...extra }]);
  assert.match(
    parseDiagram(g({ x: 0, y: 0, width: 0, height: 5 })).error,
    /nodes\[0\]/,
  );
  assert.match(
    parseDiagram(g({ x: 0, y: 0, width: 5, height: 1e9 })).error,
    /nodes\[0\]/,
  );
  const note = textNode("n", { x: 1e9, y: 0, sushiai: { note: true } });
  assert.match(parseDiagram(file([note])).error, /nodes\[0\]/);

  const t = Date.now();
  const pos = placeNodes(bare([nd("a", { x: 1e20, y: 0 }), nd("b"), nd("c")]));
  assert.ok(Date.now() - t < 500);
  assert.equal(pos.size, 3);
  const wide = {
    ...bare([nd("m", { group: "g" })]),
    groups: [{ id: "g", label: "G", x: 0, y: 0, w: 1e20, h: 1e20 }],
  };
  assert.equal(placeNodes(wide).size, 1);
});

test("500 groups tiling the lattice do not stall placeNodes", async () => {
  const { placeNodes } = await model;
  const groups = Array.from({ length: 500 }, (_, i) => ({
    id: `g${i}`,
    label: "g",
    x: 0,
    y: (499 - i) * 90 * 14,
    w: 1000,
    h: 90 * 14,
  }));
  const d = {
    ...bare(Array.from({ length: 499 }, (_, i) => nd(`n${i}`))),
    groups,
  };
  const t = Date.now();
  const pos = placeNodes(d);
  assert.ok(Date.now() - t < 300, `took ${Date.now() - t} ms`);
  assert.equal(pos.size, 499);
  assertNoOverlap(pos);
});

test("add-group and add-note append valid items and ignore the rest", async () => {
  const { applyOp } = await model;
  const d = base();
  const g = applyOp(d, {
    op: "add-group",
    group: {
      id: "phone",
      label: "Phone",
      icon: "phone",
      x: 0,
      y: 0,
      w: 200,
      h: 120,
    },
  });
  assert.equal(g.groups.length, 2);
  const badGroups = [
    { id: "cloud", label: "Dup" },
    { id: "api", label: "Clash with a node" },
    { id: "", label: "x" },
    { id: "g", label: 1 },
    { id: "g", label: "x", icon: "toaster" },
    { id: "g", label: "x", w: 0 },
    { id: "g", label: "x", h: -1 },
    { id: "g", label: "x", x: 100001 },
    { id: "g", label: "x", y: NaN },
  ];
  for (const group of badGroups) {
    assert.equal(
      applyOp(d, { op: "add-group", group }),
      d,
      JSON.stringify(group),
    );
  }
  const n = applyOp(d, {
    op: "add-note",
    note: { id: "n2", text: "q", x: 1, y: 2 },
  });
  assert.equal(n.notes.length, 2);
  const badNotes = [
    { id: "n1", text: "dup", x: 0, y: 0 },
    { id: "n", text: 1, x: 0, y: 0 },
    { id: "n", text: "t", x: 0 },
    { id: "n", text: "t", x: 0, y: -100001 },
  ];
  for (const note of badNotes) {
    assert.equal(applyOp(d, { op: "add-note", note }), d, JSON.stringify(note));
  }
});

test("move-group sets the rect and shifts only placed members", async () => {
  const { applyOp } = await model;
  const d = base();
  d.nodes[0].x = 10;
  d.nodes[0].y = 20;
  d.nodes.push({ id: "other", kind: "app", title: "O", x: 900, y: 900 });
  const rect = { x: 50, y: 60, w: 300, h: 200 };
  const out = applyOp(d, {
    op: "move-group",
    id: "cloud",
    rect,
    dx: 5,
    dy: -5,
  });
  assert.deepEqual(out.groups[0], { id: "cloud", label: "Cloud", ...rect });
  assert.deepEqual([out.nodes[0].x, out.nodes[0].y], [15, 15]);
  // db has no group, other is outside the group
  assert.equal(out.nodes[1].x, undefined);
  assert.deepEqual([out.nodes[2].x, out.nodes[2].y], [900, 900]);
  // an unplaced member stays unplaced
  const un = applyOp(base(), {
    op: "move-group",
    id: "cloud",
    rect,
    dx: 5,
    dy: 5,
  });
  assert.equal(un.nodes[0].x, undefined);
  assert.equal(un.nodes[0].y, undefined);
  // no-ops
  assert.equal(
    applyOp(d, { op: "move-group", id: "zz", rect, dx: 1, dy: 1 }),
    d,
  );
  assert.equal(
    applyOp(d, { op: "move-group", id: "cloud", rect, dx: NaN, dy: 1 }),
    d,
  );
  assert.equal(
    applyOp(d, {
      op: "move-group",
      id: "cloud",
      rect: { ...rect, w: 0 },
      dx: 1,
      dy: 1,
    }),
    d,
  );
});

test("resize-group sets the rect with a minimum size", async () => {
  const { applyOp } = await model;
  const d = base();
  const out = applyOp(d, {
    op: "resize-group",
    id: "cloud",
    rect: { x: 1, y: 2, w: 400, h: 300 },
  });
  assert.deepEqual(out.groups[0], {
    id: "cloud",
    label: "Cloud",
    x: 1,
    y: 2,
    w: 400,
    h: 300,
  });
  const small = applyOp(d, {
    op: "resize-group",
    id: "cloud",
    rect: { x: 1, y: 2, w: 10, h: 10 },
  });
  assert.deepEqual([small.groups[0].w, small.groups[0].h], [190, 100]);
  const rect = { x: 0, y: 0, w: 300, h: 200 };
  assert.equal(applyOp(d, { op: "resize-group", id: "zz", rect }), d);
  assert.equal(applyOp(d, { op: "resize-group", id: "api", rect }), d);
  assert.equal(
    applyOp(d, {
      op: "resize-group",
      id: "cloud",
      rect: { ...rect, x: Infinity },
    }),
    d,
  );
});

test("set icon on a group and set text on a note", async () => {
  const { applyOp } = await model;
  let d = applyOp(base(), {
    op: "set",
    id: "cloud",
    field: "icon",
    value: "cloud",
  });
  assert.equal(d.groups[0].icon, "cloud");
  assert.equal(
    applyOp(d, { op: "set", id: "cloud", field: "icon", value: "toaster" }),
    d,
  );
  d = applyOp(d, { op: "set", id: "cloud", field: "icon", value: "" });
  assert.ok(!("icon" in d.groups[0]));
  const b = base();
  const t = applyOp(b, { op: "set", id: "n1", field: "text", value: "new" });
  assert.equal(t.notes[0].text, "new");
  assert.equal(
    applyOp(b, { op: "set", id: "n1", field: "text", value: "" }),
    b,
  );
  assert.equal(
    applyOp(b, { op: "set", id: "api", field: "text", value: "x" }),
    b,
  );
  // group label stays non-empty
  assert.equal(
    applyOp(b, { op: "set", id: "cloud", field: "label", value: "" }),
    b,
  );
});

test("reidOps renames a clashing add-group and add-note and remaps later ops", async () => {
  const { reidOps, applyOps } = await model;
  const fresh = base();
  const ops = [
    {
      op: "add-group",
      group: { id: "cloud", label: "Mine", x: 1000, y: 0, w: 300, h: 200 },
    },
    { op: "add-note", note: { id: "n1", text: "mine", x: 0, y: 0 } },
    { op: "set", id: "api", field: "group", value: "cloud" },
    {
      op: "move-group",
      id: "cloud",
      rect: { x: 1000, y: 0, w: 300, h: 200 },
      dx: 0,
      dy: 0,
    },
    {
      op: "resize-group",
      id: "cloud",
      rect: { x: 1000, y: 0, w: 400, h: 300 },
    },
    { op: "set", id: "cloud", field: "icon", value: "server" },
    { op: "set", id: "n1", field: "text", value: "edited" },
  ];
  const re = reidOps(fresh, ops);
  assert.equal(re[0].group.id, "cloud-2");
  assert.equal(re[1].note.id, "n1-2");
  assert.equal(re[2].value, "cloud-2");
  assert.equal(re[3].id, "cloud-2");
  assert.equal(re[4].id, "cloud-2");
  assert.equal(re[5].id, "cloud-2");
  assert.equal(re[6].id, "n1-2");
  const out = applyOps(fresh, re);
  assert.equal(out.groups[0].label, "Cloud");
  assert.deepEqual(out.groups[1], {
    id: "cloud-2",
    label: "Mine",
    x: 1000,
    y: 0,
    w: 400,
    h: 300,
    icon: "server",
  });
  assert.equal(out.nodes[0].group, "cloud-2");
  assert.equal(out.notes[0].text, "why?");
  assert.equal(out.notes[1].text, "edited");
});

test("coordinates outside the file range are refused", async () => {
  const { applyOp } = await model;
  const d = {
    version: 1,
    groups: [{ id: "g", label: "G", x: 0, y: 0, w: 300, h: 200 }],
    nodes: [{ id: "a", kind: "app", title: "A", group: "g", x: 99990, y: 0 }],
    edges: [],
    notes: [],
  };
  const far = applyOp(d, {
    op: "add-node",
    node: { id: "b", kind: "app", title: "B", x: 100001, y: 0 },
  });
  assert.equal(far.nodes.length, 1);
  const ok = applyOp(d, {
    op: "add-node",
    node: { id: "b", kind: "app", title: "B", x: 100000, y: -100000 },
  });
  assert.equal(ok.nodes.length, 2);
  const moved = applyOp(d, {
    op: "move-group",
    id: "g",
    rect: { x: 20, y: 0, w: 300, h: 200 },
    dx: 20,
    dy: 0,
  });
  assert.equal(moved, d);
});

test("a later op follows a remap carried over from an earlier batch", async () => {
  const { reidOps } = await model;
  const fresh = {
    version: 1,
    groups: [{ id: "g", label: "Agent" }],
    nodes: [],
    edges: [],
    notes: [],
  };
  const remap = new Map();
  reidOps(
    fresh,
    [{ op: "add-group", group: { id: "g", label: "Mine" } }],
    remap,
  );
  assert.equal(remap.get("g"), "g-2");
  const later = reidOps(
    fresh,
    [{ op: "set", id: "g", field: "label", value: "Renamed" }],
    remap,
  );
  assert.equal(later[0].id, "g-2");
});

test("parseDiagram validates the universal block fields", async () => {
  const { parseDiagram } = await model;
  const withNode = (extra, edge = {}, own = {}) =>
    parseDiagram(
      file(
        [textNode("a", { ...extra, sushiai: own }), textNode("b", { x: 300 })],
        [{ id: "e", fromNode: "a", toNode: "b", ...edge }],
      ),
    );
  assert.ok(withNode({}).diagram, "id, type and text are enough");
  const full = withNode(
    { text: "A\nt" },
    { sushiai: { style: "dashed" }, fromEnd: "arrow" },
    {
      shape: "diamond",
      icon: "bot",
      caption: "Agent",
      step: 3,
      delta: "removed",
      kind: "app",
    },
  ).diagram;
  assert.ok(full);
  assert.equal(full.edges[0].arrow, "both");
  assert.equal(full.nodes[0].tech, "t");
  const err = (own, edge) => withNode({}, edge, own).error;
  assert.match(err({ shape: "star" }), /unknown shape/);
  assert.match(err({ icon: 5 }), /icon/);
  assert.match(err({ icon: "x".repeat(41) }), /icon/);
  assert.match(err({ caption: 5 }), /caption/);
  assert.ok(
    withNode(
      { text: `${"x".repeat(500)}\n${"y".repeat(500)}` },
      {},
      {
        caption: "x".repeat(500),
      },
    ).diagram,
  );
  assert.ok(withNode({}, {}, { icon: "not-in-the-table" }).diagram);
  for (const step of [0, 1.5, 1000, "2", -1]) {
    assert.match(err({ step }), /step/);
  }
  assert.match(err({ kind: "db" }), /unknown kind/);
  assert.match(err({ delta: "old" }), /unknown delta/);
  assert.match(err({}, { toEnd: "up" }), /unknown toEnd/);
  assert.match(err({}, { sushiai: { style: "wavy" } }), /unknown style/);
  const group = (own) =>
    parseDiagram(
      file([
        {
          id: "g",
          type: "group",
          x: 0,
          y: 0,
          width: 9,
          height: 9,
          sushiai: own,
        },
      ]),
    );
  assert.ok(group({ icon: "lock" }).diagram);
  assert.ok(group({ icon: "x" }).diagram);
  assert.equal(group({}).diagram.groups[0].label, "");
  assert.match(
    parseDiagram(file([{ id: "g", type: "group", x: 0, y: 0 }])).error,
    /width/,
  );
});

test("nodeSize gives each shape its size", async () => {
  const { nodeSize } = await model;
  const size = (shape) => nodeSize({ shape });
  assert.deepEqual(size(undefined), { w: 170, h: 62 });
  assert.deepEqual(size("box"), { w: 170, h: 62 });
  assert.deepEqual(size("pill"), { w: 170, h: 44 });
  assert.deepEqual(size("diamond"), { w: 150, h: 90 });
  assert.deepEqual(size("cylinder"), { w: 170, h: 72 });
  assert.deepEqual(size("person"), { w: 120, h: 84 });
  assert.deepEqual(size("circle"), { w: 72, h: 72 });
});

test("presetOf: explicit icon and caption win over the legacy kind", async () => {
  const { presetOf } = await model;
  assert.deepEqual(presetOf({ id: "a", title: "A", kind: "service" }), {
    icon: "server",
    caption: "Service",
  });
  assert.deepEqual(presetOf({ id: "a", title: "A", kind: "data" }), {
    icon: "database",
    caption: "Data",
  });
  assert.deepEqual(
    presetOf({ id: "a", title: "A", kind: "app", icon: "bot", caption: "X" }),
    { icon: "bot", caption: "X" },
  );
  assert.deepEqual(presetOf({ id: "a", title: "A", icon: "key" }), {
    icon: "key",
  });
  assert.deepEqual(presetOf({ id: "a", title: "A" }), {});
  // an unknown icon name draws nothing; the kind's preset still applies
  assert.deepEqual(presetOf({ id: "a", title: "A", icon: "mystery" }), {});
  assert.deepEqual(
    presetOf({ id: "a", title: "A", kind: "data", icon: "mystery" }),
    { icon: "database", caption: "Data" },
  );
});

test("PRESETS are valid blocks", async () => {
  const { PRESETS, parseDiagram } = await model;
  assert.deepEqual(
    PRESETS.map((p) => p.id),
    [
      "step",
      "start",
      "decision",
      "app",
      "service",
      "data",
      "external",
      "agent",
      "skill",
      "person",
    ],
  );
  const { serializeDiagram } = await model;
  const nodes = PRESETS.map((p) => ({ id: p.id, title: p.label, ...p.node }));
  const text = serializeDiagram({ ...bare(nodes) });
  assert.deepEqual(
    parseDiagram(text).diagram.nodes,
    nodes.map((n, i) => ({
      ...n,
      x: parseDiagram(text).diagram.nodes[i].x,
      y: parseDiagram(text).diagram.nodes[i].y,
    })),
  );
});

test("set handles the universal block fields", async () => {
  const { applyOp } = await model;
  const d = {
    version: 1,
    groups: [{ id: "g", label: "G" }],
    notes: [],
    nodes: [
      { id: "a", title: "A", step: 2 },
      { id: "b", title: "B" },
    ],
    edges: [{ id: "e", from: "a", to: "b" }],
  };
  const set = (id, field, value) => applyOp(d, { op: "set", id, field, value });
  assert.equal(set("a", "shape", "pill").nodes[0].shape, "pill");
  assert.equal(set("a", "shape", "box").nodes[0].shape, "box");
  assert.equal(set("a", "shape", "star"), d);
  assert.equal(set("a", "icon", "bot").nodes[0].icon, "bot");
  assert.equal(set("a", "icon", "nope"), d);
  assert.equal(set("a", "caption", "Skill").nodes[0].caption, "Skill");
  assert.equal(
    set("a", "caption", "x".repeat(300)).nodes[0].caption.length,
    300,
  );
  assert.equal(set("a", "step", "7").nodes[0].step, 7);
  assert.equal("step" in set("a", "step", "").nodes[0], false);
  for (const bad of ["0", "1.5", "1000", "abc", "-2"]) {
    assert.equal(set("a", "step", bad), d);
  }
  assert.equal(set("a", "delta", "removed").nodes[0].delta, "removed");
  assert.equal(set("e", "arrow", "none").edges[0].arrow, "none");
  assert.equal(set("e", "arrow", "up"), d);
  assert.equal(set("e", "style", "dashed").edges[0].style, "dashed");
  assert.equal(set("g", "icon", "lock").groups[0].icon, "lock");
});

test("placement with mixed shapes never overlaps", async () => {
  const { placeNodes, nodeSize } = await model;
  const shapes = ["box", "diamond", "person", "pill", "cylinder", "circle"];
  const nodes = [];
  for (let i = 0; i < 18; i++) {
    nodes.push({ id: `n${i}`, title: "t", shape: shapes[i % shapes.length] });
  }
  nodes.push({ id: "fixed", title: "f", shape: "diamond", x: 30, y: 50 });
  const d = { version: 1, groups: [], nodes, edges: [], notes: [] };
  const pos = placeNodes(d);
  const rects = nodes.map((n) => ({ ...pos.get(n.id), ...nodeSize(n) }));
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) {
      const a = rects[i];
      const b = rects[j];
      const apart =
        a.x + a.w <= b.x ||
        b.x + b.w <= a.x ||
        a.y + a.h <= b.y ||
        b.y + b.h <= a.y;
      assert.ok(apart, `${nodes[i].id} overlaps ${nodes[j].id}`);
    }
  }
});

test("routeEdge joins a diamond and a box", async () => {
  const { routeEdge, nodeSize } = await model;
  const diamond = { x: 0, y: 0, ...nodeSize({ shape: "diamond" }) };
  const box = { x: 0, y: 200, ...nodeSize({}) };
  assert.deepEqual(routeEdge(diamond, box), [
    { x: 75, y: 90 },
    { x: 75, y: 200 },
  ]);
  const side = { x: 300, y: 10, ...nodeSize({}) };
  const pts = routeEdge(diamond, side);
  assert.deepEqual(pts[0], { x: 150, y: 45 });
  assert.deepEqual(pts[pts.length - 1], { x: 300, y: 45 });
});

const segsOf = (pts) => pts.slice(1).map((p, i) => [pts[i], p]);
const sharedLen = ([a, b], [c, d]) => {
  const ah = a.y === b.y;
  const ch = c.y === d.y;
  if (ah !== ch) return 0;
  const [u, v] = ah ? ["x", "y"] : ["y", "x"];
  if (Math.abs(a[v] - c[v]) > 1) return 0;
  const lo = Math.max(Math.min(a[u], b[u]), Math.min(c[u], d[u]));
  const hi = Math.min(Math.max(a[u], b[u]), Math.max(c[u], d[u]));
  return hi - lo;
};
const noSharedSegments = (routes) => {
  const ids = [...routes.keys()];
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      for (const s of segsOf(routes.get(ids[i]))) {
        for (const t of segsOf(routes.get(ids[j]))) {
          assert.ok(
            sharedLen(s, t) <= 4,
            `${ids[i]} and ${ids[j]} share a segment`,
          );
        }
      }
    }
  }
};

test("routeEdges: an antiparallel pair gets two separate lines", async () => {
  const { routeEdges } = await model;
  const d = {
    version: 1,
    groups: [],
    notes: [],
    nodes: [
      { id: "a", title: "A" },
      { id: "b", title: "B" },
    ],
    edges: [
      { id: "ab", from: "a", to: "b" },
      { id: "ba", from: "b", to: "a" },
    ],
  };
  const pos = new Map([
    ["a", { x: 0, y: 0 }],
    ["b", { x: 0, y: 200 }],
  ]);
  const r = routeEdges(d, pos);
  assert.notDeepEqual(r.get("ab"), r.get("ba"));
  assert.equal(r.get("ab")[0].x !== r.get("ba")[0].x, true);
  noSharedSegments(r);
  assert.deepEqual(r.get("ab"), [
    { x: 77, y: 62 },
    { x: 77, y: 200 },
  ]);
});

test("routeEdges: three edges on one side use three ports", async () => {
  const { routeEdges } = await model;
  const d = {
    version: 1,
    groups: [],
    notes: [],
    nodes: [
      { id: "a", title: "A" },
      { id: "b", title: "B" },
      { id: "c", title: "C" },
      { id: "e", title: "E" },
    ],
    edges: [
      { id: "ab", from: "a", to: "b" },
      { id: "ac", from: "a", to: "c" },
      { id: "ae", from: "a", to: "e" },
    ],
  };
  const pos = new Map([
    ["a", { x: 200, y: 0 }],
    ["b", { x: 0, y: 150 }],
    ["c", { x: 200, y: 300 }],
    ["e", { x: 400, y: 150 }],
  ]);
  const r = routeEdges(d, pos);
  const starts = ["ab", "ac", "ae"].map((id) => r.get(id)[0]);
  assert.equal(new Set(starts.map((p) => `${p.x},${p.y}`)).size, 3);
  noSharedSegments(r);
});

test("routeEdges: a diamond sends two edges out of different vertices", async () => {
  const { routeEdges } = await model;
  const d = {
    version: 1,
    groups: [],
    notes: [],
    nodes: [
      { id: "q", title: "Q?", shape: "diamond" },
      { id: "yes", title: "Y" },
      { id: "no", title: "N" },
    ],
    edges: [
      { id: "y", from: "q", to: "yes" },
      { id: "n", from: "q", to: "no" },
    ],
  };
  const pos = new Map([
    ["q", { x: 200, y: 0 }],
    ["yes", { x: 500, y: 20 }],
    ["no", { x: 200, y: 200 }],
  ]);
  const r = routeEdges(d, pos);
  // vertices of the diamond: right (350, 45) and bottom (275, 90)
  assert.deepEqual(r.get("y")[0], { x: 350, y: 45 });
  assert.deepEqual(r.get("n")[0], { x: 275, y: 90 });
  noSharedSegments(r);
});

test("routeEdges: two edges toward the same side of a diamond take two vertices", async () => {
  const { routeEdges } = await model;
  const d = {
    version: 1,
    groups: [],
    notes: [],
    nodes: [
      { id: "a", title: "A" },
      { id: "q", title: "Q?", shape: "diamond" },
    ],
    edges: [
      { id: "in", from: "a", to: "q" },
      { id: "back", from: "q", to: "a", label: "no" },
    ],
  };
  const pos = new Map([
    ["a", { x: 200, y: 0 }],
    ["q", { x: 200, y: 200 }],
  ]);
  const r = routeEdges(d, pos);
  assert.notDeepEqual(r.get("in").at(-1), r.get("back")[0]);
  noSharedSegments(r);
});

test("routeEdges: no two edges of the agent-flow fixture share a segment", async () => {
  const { parseDiagram, placeNodes, routeEdges } = await model;
  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures/agent-flow.canvas"),
    "utf8",
  );
  const d = parseDiagram(text).diagram;
  const routes = routeEdges(d, placeNodes(d));
  assert.equal(routes.size, d.edges.length);
  for (const pts of routes.values()) assert.ok(pts.length >= 2);
  noSharedSegments(routes);
  // at most one crossing between all the routes together
  const all = [...routes.values()];
  let crossings = 0;
  for (let i = 0; i < all.length; i++) {
    for (let j = i + 1; j < all.length; j++) {
      for (const [a, b] of segsOf(all[i])) {
        for (const [c, e] of segsOf(all[j])) {
          const ah = a.y === b.y;
          if (ah === (c.y === e.y)) continue;
          const [h1, h2, v1, v2] = ah ? [a, b, c, e] : [c, e, a, b];
          if (
            Math.min(h1.x, h2.x) < v1.x &&
            v1.x < Math.max(h1.x, h2.x) &&
            Math.min(v1.y, v2.y) < h1.y &&
            h1.y < Math.max(v1.y, v2.y)
          ) {
            crossings++;
          }
        }
      }
    }
  }
  assert.ok(crossings <= 1, `${crossings} crossings`);
});

test("routeEdges keeps lone links exactly as routeEdge draws them", async () => {
  const {
    parseDiagram,
    placeNodes,
    groupRects,
    nodeSize,
    routeEdge,
    routeEdges,
  } = await model;
  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures/shop.canvas"),
    "utf8",
  );
  const d = parseDiagram(text).diagram;
  const pos = placeNodes(d);
  const avoid = [...groupRects(d).values()];
  const routes = routeEdges(d, pos, avoid);
  const rect = (id) => ({
    ...pos.get(id),
    ...nodeSize(d.nodes.find((n) => n.id === id)),
  });
  let same = 0;
  for (const e of d.edges) {
    const want = routeEdge(rect(e.from), rect(e.to), avoid);
    if (JSON.stringify(routes.get(e.id)) === JSON.stringify(want)) same++;
  }
  assert.equal(same, d.edges.length);
});

const flow = (nodes, edges) => ({
  version: 1,
  groups: [],
  notes: [],
  nodes,
  edges,
});

test("routeEdges: ends sit on the outline of each shape", async () => {
  const { routeEdges } = await model;
  const d = flow(
    [
      { id: "b1", title: "B" },
      { id: "dia", title: "D", shape: "diamond" },
      { id: "b2", title: "B" },
      { id: "cir", title: "C", shape: "circle" },
      { id: "b3", title: "B" },
      { id: "pil", title: "P", shape: "pill" },
      { id: "b4", title: "B" },
      { id: "cyl", title: "Y", shape: "cylinder" },
    ],
    [
      { id: "to-dia", from: "b1", to: "dia" },
      { id: "to-cir", from: "b2", to: "cir" },
      { id: "to-pil", from: "b3", to: "pil" },
      { id: "to-cyl", from: "b4", to: "cyl" },
    ],
  );
  const pos = new Map([
    ["b1", { x: 0, y: 0 }],
    ["dia", { x: 400, y: 0 }],
    ["b2", { x: 0, y: 300 }],
    ["cir", { x: 400, y: 300 }],
    ["b3", { x: 0, y: 600 }],
    ["pil", { x: 400, y: 600 }],
    ["b4", { x: 0, y: 900 }],
    ["cyl", { x: 0, y: 1100 }],
  ]);
  const r = routeEdges(d, pos);
  const last = (id) => r.get(id).at(-1);
  // left vertex of the diamond
  assert.deepEqual(last("to-dia"), { x: 400, y: 45 });
  // a point of the circle (centre 436, 336, radius 36) on its left half
  const c = last("to-cir");
  assert.ok(Math.abs(Math.hypot(c.x - 436, c.y - 336) - 36) < 1e-9);
  assert.ok(c.x < 436);
  // the rounded left end of a pill (cap centre 422, 622, radius 22)
  const q = last("to-pil");
  assert.ok(Math.abs(Math.hypot(q.x - 422, q.y - 622) - 22) < 1e-9);
  assert.ok(q.x < 422);
  // top of a cylinder is 6px below its bounding box
  assert.deepEqual(last("to-cyl"), { x: 85, y: 1106 });
});

test("routeEdges: a diamond end is its vertex even with a neighbour", async () => {
  const { routeEdges } = await model;
  const d = flow(
    [
      { id: "p", title: "P", shape: "pill" },
      { id: "q", title: "Q", shape: "diamond" },
    ],
    [{ id: "e", from: "p", to: "q" }],
  );
  const pos = new Map([
    ["p", { x: 0, y: 0 }],
    ["q", { x: 300, y: 20 }],
  ]);
  const pts = routeEdges(d, pos).get("e");
  // left vertex: x 300, y 20 + 45
  assert.deepEqual(pts.at(-1), { x: 300, y: 65 });
  assert.deepEqual(pts[0], { x: 170, y: 22 });
});

test("routeEdges: a run keeps 12px from blocks that are not its ends", async () => {
  const { routeEdges, nodeSize } = await model;
  const d = flow(
    [
      { id: "q", title: "Q?", shape: "diamond" },
      { id: "wall", title: "W" },
      { id: "dest", title: "D" },
    ],
    [{ id: "yes", from: "q", to: "dest" }],
  );
  const pos = new Map([
    ["q", { x: 0, y: 100 }],
    ["wall", { x: 0, y: 0 }],
    ["dest", { x: 190, y: 0 }],
  ]);
  const pts = routeEdges(d, pos).get("yes");
  const wall = { x: 0, y: 0, ...nodeSize({}) };
  for (let i = 1; i < pts.length; i++) {
    const [p, q] = [pts[i - 1], pts[i]];
    if (p.x !== q.x) continue; // only vertical runs can graze the wall
    const overlapsY =
      Math.min(wall.y + wall.h, Math.max(p.y, q.y)) >
      Math.max(wall.y, Math.min(p.y, q.y));
    if (overlapsY) assert.ok(p.x >= wall.x + wall.w + 12, `x ${p.x} too close`);
  }
});

test("routeEdges: an antiparallel pair of wider and narrower blocks stays straight", async () => {
  const { routeEdges } = await model;
  const d = flow(
    [
      { id: "a", title: "A" },
      { id: "b", title: "B", shape: "pill" },
    ],
    [
      { id: "ab", from: "a", to: "b" },
      { id: "ba", from: "b", to: "a" },
    ],
  );
  const pos = new Map([
    ["a", { x: 0, y: 0 }],
    ["b", { x: 20, y: 200 }],
  ]);
  const r = routeEdges(d, pos);
  // shared centre line is x 85 (a's centre is inside b's range)
  assert.equal(r.get("ab").length, 2);
  assert.equal(r.get("ba").length, 2);
  assert.equal(r.get("ab")[0].x, 77);
  assert.equal(r.get("ba")[0].x, 93);
  assert.equal(r.get("ab")[0].x, r.get("ab")[1].x);
  assert.equal(r.get("ba")[0].x, r.get("ba")[1].x);
});

test("routeEdges: no route runs through a block, ends included", async () => {
  const { parseDiagram, placeNodes, routeEdges, nodeSize } = await model;
  const inside = (n, p, at) => {
    const { w, h } = nodeSize(n);
    const dx = Math.abs(at.x - (p.x + w / 2));
    const dy = Math.abs(at.y - (p.y + h / 2));
    if (n.shape === "diamond") return dx / (w / 2 - 1) + dy / (h / 2 - 1) < 1;
    if (n.shape === "circle") return Math.hypot(dx, dy) < w / 2 - 1;
    if (n.shape === "pill") {
      const r = h / 2;
      return Math.hypot(Math.max(dx - (w / 2 - r), 0), dy) < r - 1;
    }
    return dx < w / 2 - 1 && dy < h / 2 - 1;
  };
  const check = (name, d, pos = placeNodes(d)) => {
    const routes = routeEdges(d, pos);
    assert.equal(routes.size, d.edges.length, name);
    for (const [id, pts] of routes) {
      for (let i = 1; i < pts.length; i++) {
        const [a, b] = [pts[i - 1], pts[i]];
        const steps = Math.max(1, Math.ceil(Math.hypot(b.x - a.x, b.y - a.y)));
        for (let k = 0; k <= steps; k++) {
          const at = {
            x: a.x + ((b.x - a.x) * k) / steps,
            y: a.y + ((b.y - a.y) * k) / steps,
          };
          for (const n of d.nodes) {
            assert.ok(
              !inside(n, pos.get(n.id), at),
              `${name}: ${id} runs through ${n.id} at ${at.x},${at.y}`,
            );
          }
        }
      }
    }
  };
  const read = (f) =>
    parseDiagram(
      require("node:fs").readFileSync(
        require("node:path").join(__dirname, "fixtures", f),
        "utf8",
      ),
    ).diagram;
  check("agent-flow", read("agent-flow.canvas"));
  check("shop", read("shop.canvas"));
  const star = flow(
    [
      { id: "q", title: "Q?", shape: "diamond" },
      { id: "n", title: "N" },
      { id: "e", title: "E" },
      { id: "s", title: "S" },
      { id: "w", title: "W" },
    ],
    ["n", "e", "s", "w"].map((to) => ({ id: `q-${to}`, from: "q", to })),
  );
  const at = new Map([
    ["q", { x: 400, y: 400 }],
    ["n", { x: 400, y: 100 }],
    ["e", { x: 700, y: 400 }],
    ["s", { x: 400, y: 700 }],
    ["w", { x: 100, y: 400 }],
  ]);
  check("star", star, at);
  const r = routeEdges(star, at);
  const first = ["n", "e", "s", "w"].map((to) => r.get(`q-${to}`)[0]);
  assert.equal(new Set(first.map((p) => `${p.x},${p.y}`)).size, 4);
});

test("routeEdges: an edge entering the side of a kept line gets its own port", async () => {
  const { routeEdges } = await model;
  const d = flow(
    [
      { id: "a", title: "A" },
      { id: "q", title: "Q?", shape: "diamond" },
      { id: "t", title: "T" },
    ],
    [
      { id: "straight", from: "a", to: "t" },
      { id: "yes", from: "q", to: "t" },
    ],
  );
  const pos = new Map([
    ["a", { x: 0, y: 0 }],
    ["q", { x: 0, y: 200 }],
    ["t", { x: 400, y: 0 }],
  ]);
  const r = routeEdges(d, pos);
  assert.notDeepEqual(r.get("straight").at(-1), r.get("yes").at(-1));
  const [s, y] = [r.get("straight"), r.get("yes")];
  assert.ok(Math.abs(s.at(-1).y - y.at(-1).y) > 4);
});

test("routeEdges: every segment is horizontal or vertical", async () => {
  const { parseDiagram, placeNodes, routeEdges } = await model;
  const read = (f) =>
    parseDiagram(
      require("node:fs").readFileSync(
        require("node:path").join(__dirname, "fixtures", f),
        "utf8",
      ),
    ).diagram;
  const mixed = flow(
    [
      { id: "p1", title: "P", shape: "pill", x: 0, y: 0 },
      { id: "p2", title: "Q", shape: "pill", x: 20, y: 200 },
      { id: "c1", title: "C", shape: "circle", x: 300, y: 0 },
      { id: "c2", title: "D", shape: "circle", x: 330, y: 200 },
      { id: "y1", title: "Y", shape: "cylinder", x: 600, y: 0 },
      { id: "y2", title: "Z", shape: "cylinder", x: 650, y: 200 },
    ],
    [
      { id: "pp", from: "p1", to: "p2" },
      { id: "cc", from: "c1", to: "c2" },
      { id: "yy", from: "y1", to: "y2" },
      { id: "pc", from: "p2", to: "c1" },
    ],
  );
  for (const d of [read("agent-flow.canvas"), read("shop.canvas"), mixed]) {
    for (const [id, pts] of routeEdges(d, placeNodes(d))) {
      for (let i = 1; i < pts.length; i++) {
        assert.ok(
          pts[i].x === pts[i - 1].x || pts[i].y === pts[i - 1].y,
          `${id} has a slanted segment`,
        );
      }
    }
  }
});

const fixtureRoutes = async (name, nodesOnly) => {
  const { parseDiagram, placeNodes, nodeSize, routeEdges } = await model;
  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures", name),
    "utf8",
  );
  const d = parseDiagram(text).diagram;
  if (nodesOnly) {
    d.nodes = d.nodes.filter((n) => nodesOnly.includes(n.id));
    d.edges = d.edges.filter(
      (e) => nodesOnly.includes(e.from) && nodesOnly.includes(e.to),
    );
  }
  const pos = placeNodes(d);
  const rects = new Map(
    d.nodes.map((n) => [n.id, { ...pos.get(n.id), ...nodeSize(n) }]),
  );
  return { d, rects, routes: routeEdges(d, pos) };
};

const clearOfOthers = ({ d, rects, routes }) => {
  for (const e of d.edges) {
    const pts = routes.get(e.id);
    for (const [id, r] of rects) {
      if (id === e.from || id === e.to) continue;
      for (const [a, b] of segsOf(pts)) {
        const inside =
          Math.max(a.x, b.x) > r.x - 12 &&
          Math.min(a.x, b.x) < r.x + r.w + 12 &&
          Math.max(a.y, b.y) > r.y - 12 &&
          Math.min(a.y, b.y) < r.y + r.h + 12;
        assert.ok(!inside, `${e.id} comes within 12px of ${id}`);
      }
    }
  }
};

test("routeEdges: no run comes within 12px of a block that is not its end", async () => {
  for (const name of ["agent-flow.canvas", "shop.canvas"]) {
    clearOfOthers(await fixtureRoutes(name));
  }
});

test("routeEdges: a route never squeezes through a corridor narrower than the clearance", async () => {
  const { nodeSize, routeEdges } = await model;
  // A and B stand 21px apart; the diamond below B leads back to A
  const d = flow(
    [
      { id: "a", title: "A" },
      { id: "b", title: "B" },
      { id: "q", title: "Q", shape: "diamond" },
    ],
    [{ id: "e", from: "q", to: "a" }],
  );
  const pos = new Map([
    ["a", { x: 0, y: 0 }],
    ["b", { x: nodeSize(d.nodes[0]).w + 21, y: 0 }],
    ["q", { x: nodeSize(d.nodes[0]).w + 21, y: 160 }],
  ]);
  const rects = new Map(
    d.nodes.map((n) => [n.id, { ...pos.get(n.id), ...nodeSize(n) }]),
  );
  clearOfOthers({ d, rects, routes: routeEdges(d, pos) });
});

test("routeEdges: agent-flow has no sideways step under 16px and diamonds end on vertices", async () => {
  const { d, rects, routes } = await fixtureRoutes("agent-flow.canvas");
  for (const e of d.edges) {
    const pts = routes.get(e.id);
    for (let i = 1; i < pts.length - 2; i++) {
      const len =
        Math.abs(pts[i + 1].x - pts[i].x) + Math.abs(pts[i + 1].y - pts[i].y);
      assert.ok(len >= 16, `${e.id} steps ${len}px sideways`);
    }
    for (const [id, p] of [
      [e.from, pts[0]],
      [e.to, pts[pts.length - 1]],
    ]) {
      if (d.nodes.find((n) => n.id === id).shape !== "diamond") continue;
      const r = rects.get(id);
      const cx = r.x + r.w / 2;
      const cy = r.y + r.h / 2;
      const vertex =
        (p.x === cx && (p.y === r.y || p.y === r.y + r.h)) ||
        (p.y === cy && (p.x === r.x || p.x === r.x + r.w));
      assert.ok(vertex, `${e.id} meets ${id} off a vertex`);
    }
  }
});

// A JSON Canvas 1.0 checker: what any tool reading the file may rely on.
function validateCanvas(text) {
  const errors = [];
  const o = JSON.parse(text);
  const ids = new Set();
  const int = (v) => Number.isInteger(v);
  const color = (v) => v === undefined || /^([1-6]|#[0-9a-fA-F]{6})$/.test(v);
  const nodeIds = new Set((o.nodes ?? []).map((n) => n.id));
  for (const n of o.nodes ?? []) {
    if (typeof n.id !== "string" || ids.has(n.id)) errors.push(`id ${n.id}`);
    ids.add(n.id);
    if (!["text", "file", "link", "group"].includes(n.type))
      errors.push(`type ${n.id}`);
    for (const k of ["x", "y", "width", "height"])
      if (!int(n[k])) errors.push(`${k} ${n.id}`);
    if (!color(n.color)) errors.push(`color ${n.id}`);
    if (n.type === "text" && typeof n.text !== "string")
      errors.push(`text ${n.id}`);
    if (n.type === "file" && typeof n.file !== "string")
      errors.push(`file ${n.id}`);
    if (n.type === "link" && typeof n.url !== "string")
      errors.push(`url ${n.id}`);
    if (
      n.type === "group" &&
      n.label !== undefined &&
      typeof n.label !== "string"
    )
      errors.push(`label ${n.id}`);
  }
  for (const e of o.edges ?? []) {
    if (typeof e.id !== "string" || ids.has(e.id)) errors.push(`id ${e.id}`);
    ids.add(e.id);
    if (!nodeIds.has(e.fromNode) || !nodeIds.has(e.toNode))
      errors.push(`ref ${e.id}`);
    for (const k of ["fromSide", "toSide"])
      if (
        e[k] !== undefined &&
        !["top", "right", "bottom", "left"].includes(e[k])
      )
        errors.push(`${k} ${e.id}`);
    for (const k of ["fromEnd", "toEnd"])
      if (e[k] !== undefined && !["none", "arrow"].includes(e[k]))
        errors.push(`${k} ${e.id}`);
    if (!color(e.color)) errors.push(`color ${e.id}`);
    if (e.label !== undefined && typeof e.label !== "string")
      errors.push(`label ${e.id}`);
  }
  return errors;
}

const fixture = (name) =>
  require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures", name),
    "utf8",
  );

test("both fixtures round-trip and are valid JSON Canvas", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  for (const name of ["shop.canvas", "agent-flow.canvas"]) {
    const first = parseDiagram(fixture(name)).diagram;
    const text = serializeDiagram(first);
    assert.deepEqual(validateCanvas(text), [], name);
    assert.deepEqual(parseDiagram(text).diagram, first, name);
    assert.equal(serializeDiagram(parseDiagram(text).diagram), text, name);
  }
});

test("a written file is valid even for an unplaced, grouped, edited diagram", async () => {
  const { applyOps, serializeDiagram, parseDiagram } = await model;
  const d = applyOps(bare([]), [
    {
      op: "add-group",
      group: { id: "g", label: "G", x: 0, y: 0, w: 300, h: 160 },
    },
    {
      op: "add-node",
      node: { id: "a", title: "A", tech: "t", delta: "new", group: "g" },
    },
    {
      op: "add-node",
      node: { id: "b", title: "B", shape: "diamond", step: 2 },
    },
    {
      op: "add-node",
      node: { id: "c", title: "C", delta: "changed", x: 10.4, y: 400.6 },
    },
    { op: "add-note", note: { id: "n", text: "why?", x: 1.5, y: 2 } },
    {
      op: "connect",
      edge: { id: "e1", from: "a", to: "b", arrow: "both", style: "guess" },
    },
    {
      op: "connect",
      edge: { id: "e2", from: "b", to: "c", arrow: "none", status: "proposed" },
    },
    { op: "connect", edge: { id: "e3", from: "a", to: "c", label: "x" } },
  ]);
  const text = serializeDiagram(d);
  assert.deepEqual(validateCanvas(text), []);
  const out = JSON.parse(text);
  const byId = (id) => out.nodes.find((n) => n.id === id);
  assert.equal(byId("a").color, "4");
  assert.equal(byId("c").color, "3");
  assert.equal(byId("b").color, undefined);
  assert.equal(byId("n").color, "3");
  assert.equal(byId("g").type, "group");
  assert.equal(byId("a").text, "A\nt");
  const edge = (id) => out.edges.find((e) => e.id === id);
  assert.deepEqual([edge("e1").fromEnd, edge("e1").toEnd], ["arrow", "arrow"]);
  assert.equal(edge("e1").color, "3");
  assert.deepEqual([edge("e2").fromEnd, edge("e2").toEnd], ["none", "none"]);
  assert.deepEqual(
    [edge("e3").fromEnd, edge("e3").toEnd],
    [undefined, undefined],
  );
  assert.equal(byId("g").label, "G");
  assert.equal(out.nodes[0].id, "g", "groups sit under the blocks");
  const back = parseDiagram(text).diagram;
  assert.equal(back.nodes.find((n) => n.id === "a").group, "g");
  assert.equal(back.nodes.find((n) => n.id === "a").delta, "new");
  assert.equal(back.edges.find((e) => e.id === "e1").style, "guess");
  assert.equal(back.edges.find((e) => e.id === "e2").status, "proposed");
});

test("a canvas made in Obsidian, with no sushiai keys, opens", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  const text = JSON.stringify({
    nodes: [
      {
        id: "g1",
        type: "group",
        label: "Backend",
        x: 0,
        y: 0,
        width: 600,
        height: 400,
        color: "5",
      },
      {
        id: "t1",
        type: "text",
        text: "# Gateway\nnginx",
        x: 40,
        y: 60,
        width: 250,
        height: 60,
        color: "4",
      },
      {
        id: "t2",
        type: "text",
        text: "Worker",
        x: 320,
        y: 60,
        width: 250,
        height: 60,
        color: "3",
      },
      {
        id: "t3",
        type: "text",
        text: "Old queue",
        x: 40,
        y: 600,
        width: 250,
        height: 60,
        color: "1",
      },
      {
        id: "t4",
        type: "text",
        text: "Plain",
        x: 700,
        y: 60,
        width: 250,
        height: 60,
        color: "#ff8800",
      },
      {
        id: "f1",
        type: "file",
        file: "docs/notes/design.md",
        x: 40,
        y: 220,
        width: 400,
        height: 200,
      },
      {
        id: "l1",
        type: "link",
        url: "https://example.com/spec",
        x: 700,
        y: 220,
        width: 400,
        height: 200,
      },
    ],
    edges: [
      {
        id: "e1",
        fromNode: "t1",
        fromSide: "right",
        toNode: "t2",
        toSide: "left",
        label: "calls",
      },
      { id: "e2", fromNode: "t2", toNode: "f1", toEnd: "none" },
      {
        id: "e3",
        fromNode: "t4",
        toNode: "l1",
        fromEnd: "arrow",
        toEnd: "arrow",
        color: "2",
      },
      { id: "e4", fromNode: "g1", toNode: "t4" },
    ],
  });
  const { diagram } = parseDiagram(text);
  const node = (id) => diagram.nodes.find((n) => n.id === id);
  assert.deepEqual(
    diagram.groups.map((g) => [g.id, g.label]),
    [["g1", "Backend"]],
  );
  assert.equal(node("t1").title, "# Gateway");
  assert.equal(node("t1").tech, "nginx");
  assert.equal(node("t1").delta, "new");
  assert.equal(node("t2").delta, "changed");
  assert.equal(node("t3").delta, "removed");
  assert.equal(node("t4").delta, undefined);
  assert.equal(node("t1").group, "g1");
  assert.equal(node("t3").group, undefined, "outside the group");
  assert.equal(node("t4").group, undefined);
  assert.equal(node("f1").title, "design.md");
  assert.equal(node("f1").caption, "File");
  assert.equal(node("l1").title, "https://example.com/spec");
  assert.equal(node("l1").caption, "Link");
  assert.equal(
    diagram.edges.length,
    3,
    "the edge from a group is kept, not drawn",
  );
  assert.equal(diagram.edges[2].arrow, "both");
  const out = JSON.parse(serializeDiagram(diagram));
  assert.deepEqual(validateCanvas(JSON.stringify(out)), []);
  const find = (id) => out.nodes.find((n) => n.id === id);
  assert.equal(find("f1").file, "docs/notes/design.md");
  assert.equal(find("f1").type, "file");
  assert.equal(find("l1").url, "https://example.com/spec");
  assert.equal(find("t4").color, "#ff8800");
  assert.equal(find("g1").color, "5");
  assert.equal(find("t1").color, "4");
  assert.equal(find("t1").text, "# Gateway\nnginx");
  const e1 = out.edges.find((e) => e.id === "e1");
  assert.deepEqual([e1.fromSide, e1.toSide], ["right", "left"]);
  assert.equal(out.edges.find((e) => e.id === "e3").color, "2");
  assert.equal(out.edges.find((e) => e.id === "e4").fromNode, "g1");
  assert.deepEqual(
    out.nodes.map((n) => n.id),
    ["g1", "t1", "t2", "t3", "t4", "f1", "l1"],
  );
});

test("a file or link node keeps its text: a title edit is refused", async () => {
  const { parseDiagram, applyOp } = await model;
  const d = parseDiagram(
    file([
      {
        id: "f",
        type: "file",
        file: "a/b.md",
        x: 0,
        y: 0,
        width: 4,
        height: 4,
      },
      textNode("t"),
    ]),
  ).diagram;
  const set = (id) => applyOp(d, { op: "set", id, field: "title", value: "X" });
  assert.equal(set("f"), d);
  assert.equal(set("t").nodes[1].title, "X");
});

test("group membership follows geometry, the smallest group wins", async () => {
  const { parseDiagram } = await model;
  const g = (id, x, y, w, h) => ({
    id,
    type: "group",
    label: id,
    x,
    y,
    width: w,
    height: h,
  });
  // a block is 170 x 62: its centre is (x + 85, y + 31)
  const { diagram } = parseDiagram(
    file([
      g("outer", 0, 0, 1000, 800),
      g("inner", 100, 100, 400, 300),
      g("deep", 150, 150, 200, 100),
      textNode("a", { x: 20, y: 20 }),
      textNode("b", { x: 120, y: 300 }),
      textNode("c", { x: 140, y: 170 }),
      textNode("d", { x: 1200, y: 20 }),
      textNode("e", { x: 600, y: 500 }),
    ]),
  );
  const of = (id) => diagram.nodes.find((n) => n.id === id).group;
  assert.equal(of("a"), "outer");
  assert.equal(of("b"), "inner");
  assert.equal(of("c"), "deep");
  assert.equal(of("d"), undefined);
  assert.equal(of("e"), "outer");
});

test("set group moves the block into the group", async () => {
  const { parseDiagram, applyOp, groupRects, serializeDiagram } = await model;
  const g = (id, x, y, w, h) => ({
    id,
    type: "group",
    label: id,
    x,
    y,
    width: w,
    height: h,
  });
  const d = parseDiagram(
    file([
      g("g1", 0, 0, 300, 200),
      g("g2", 600, 0, 300, 200),
      textNode("in", { x: 20, y: 60 }),
      textNode("out", { x: 400, y: 600 }),
    ]),
  ).diagram;
  const node = (dd, id) => dd.nodes.find((n) => n.id === id);
  // already inside: stays put
  const same = applyOp(d, { op: "set", id: "in", field: "group", value: "g1" });
  assert.deepEqual([node(same, "in").x, node(same, "in").y], [20, 60]);
  // from outside: lands inside the group's rect
  const moved = applyOp(d, {
    op: "set",
    id: "out",
    field: "group",
    value: "g2",
  });
  const at = node(moved, "out");
  assert.equal(at.group, "g2");
  const rect = groupRects(moved).get("g2");
  assert.ok(at.x >= rect.x && at.x + 170 <= rect.x + rect.w);
  assert.ok(at.y >= rect.y && at.y + 62 <= rect.y + rect.h);
  // from group to group: leaves g1, joins g2
  const hop = applyOp(d, { op: "set", id: "in", field: "group", value: "g2" });
  assert.equal(
    node(parseDiagram(serializeDiagram(hop)).diagram, "in").group,
    "g2",
  );
  assert.equal(
    node(parseDiagram(serializeDiagram(moved)).diagram, "out").group,
    "g2",
  );
  // a full group grows to hold one more
  const small = parseDiagram(
    file([
      g("g", 0, 0, 200, 100),
      textNode("a", { x: 10, y: 30 }),
      textNode("b", { x: 900, y: 900 }),
    ]),
  ).diagram;
  const grown = applyOp(small, {
    op: "set",
    id: "b",
    field: "group",
    value: "g",
  });
  const back = parseDiagram(serializeDiagram(grown)).diagram;
  assert.equal(node(back, "b").group, "g");
  assert.equal(node(back, "a").group, "g");
  // out of the group: just below it
  const left = applyOp(d, { op: "set", id: "in", field: "group", value: "" });
  assert.equal(node(left, "in").group, undefined);
  assert.equal(
    node(parseDiagram(serializeDiagram(left)).diagram, "in").group,
    undefined,
  );
  // a block dragged out first stays where it was dropped
  const dropped = applyOp(d, { op: "move", id: "in", x: 400, y: 400 });
  const free = applyOp(dropped, {
    op: "set",
    id: "in",
    field: "group",
    value: "",
  });
  assert.deepEqual([node(free, "in").x, node(free, "in").y], [400, 400]);
});

const grp = (id, x, y, width, height, extra = {}) => ({
  id,
  type: "group",
  label: id,
  x,
  y,
  width,
  height,
  ...extra,
});
const nodesOf = (text) => JSON.parse(text).nodes;

test("an Obsidian canvas survives an unrelated edit node for node", async () => {
  const { parseDiagram, serializeDiagram, applyOps } = await model;
  const original = [
    textNode("t", {
      text: "# Title\n\nPara one\n",
      x: 10,
      y: 10,
      width: 250,
      height: 180,
    }),
    {
      id: "f",
      type: "file",
      file: "a/b.md",
      x: 300,
      y: 10,
      width: 400,
      height: 400,
    },
    textNode("m", { text: "Move me", x: 900, y: 10, width: 250, height: 60 }),
  ];
  const d = parseDiagram(file(original)).diagram;
  const out = nodesOf(
    serializeDiagram(applyOps(d, [{ op: "move", id: "m", x: 920, y: 30 }])),
  );
  assert.deepEqual(out[0], original[0]);
  assert.deepEqual(out[1], original[1]);
  assert.deepEqual(
    [out[2].x, out[2].y, out[2].width, out[2].height],
    [920, 30, 250, 60],
  );
  const again = serializeDiagram(parseDiagram(serializeDiagram(d)).diagram);
  assert.equal(again, serializeDiagram(d));
  // an edit of the title writes the new text; a shape change resets the size
  const edited = applyOps(d, [
    { op: "set", id: "t", field: "title", value: "New" },
    { op: "set", id: "m", field: "shape", value: "diamond" },
  ]);
  const text = nodesOf(serializeDiagram(edited));
  assert.equal(text[0].text, "New\nPara one");
  assert.deepEqual([text[2].width, text[2].height], [150, 90]);
  // a chosen size decides which group a block belongs to
  const wide = parseDiagram(
    file([
      grp("g", 0, 0, 300, 300),
      textNode("w", {
        x: 200,
        y: 0,
        width: 400,
        height: 40,
        sushiai: { size: "fixed" },
      }),
    ]),
  ).diagram;
  assert.equal(
    wide.nodes[0].group,
    undefined,
    "its centre, by the chosen size, is outside",
  );
});

test("a group keeps the bounds the file gave it", async () => {
  const { parseDiagram, serializeDiagram, applyOps } = await model;
  const d = parseDiagram(
    file([
      grp("g", 0, 0, 300, 200),
      textNode("a", { x: 0, y: 0 }),
      textNode("b", { x: 200, y: 150 }),
    ]),
  ).diagram;
  const group = (dd) => nodesOf(serializeDiagram(dd))[0];
  const bounds = (g) => [g.x, g.y, g.width, g.height];
  assert.deepEqual(bounds(group(d)), [0, 0, 300, 200]);
  assert.deepEqual(bounds(group(applyOps(d, []))), [0, 0, 300, 200]);
  const moved = applyOps(d, [{ op: "move", id: "a", x: 20, y: 40 }]);
  assert.deepEqual(bounds(group(moved)), [0, 0, 300, 200]);
});

test("membership is derived again after ops", async () => {
  const { parseDiagram, applyOps } = await model;
  const d = parseDiagram(
    file([
      grp("g", 0, 0, 300, 200),
      textNode("a", { x: 20, y: 40 }),
      textNode("b", { x: 600, y: 40 }),
    ]),
  ).diagram;
  const node = (dd, id) => dd.nodes.find((n) => n.id === id);
  const out = applyOps(d, [
    { op: "move", id: "a", x: 900, y: 900 },
    { op: "move", id: "b", x: 40, y: 60 },
  ]);
  assert.equal(node(out, "a").group, undefined);
  assert.equal(node(out, "b").group, "g");
  assert.deepEqual(
    out.groups[0],
    d.groups[0],
    "a member moving never changes the group",
  );
});

test("moving a group carries nested groups, blocks, notes and file nodes", async () => {
  const { parseDiagram, applyOp, serializeDiagram } = await model;
  const d = parseDiagram(
    file([
      grp("outer", 0, 0, 800, 600),
      grp("inner", 400, 100, 300, 200),
      textNode("in", { x: 20, y: 40 }),
      textNode("deep", { x: 420, y: 140 }),
      {
        id: "f",
        type: "file",
        file: "a.md",
        x: 20,
        y: 300,
        width: 200,
        height: 100,
      },
      textNode("n", {
        x: 20,
        y: 500,
        width: 280,
        height: 30,
        color: "3",
        sushiai: { note: true },
      }),
      textNode("away", { x: 1200, y: 40 }),
    ]),
  ).diagram;
  const out = applyOp(d, {
    op: "move-group",
    id: "outer",
    rect: { x: 10, y: 20, w: 800, h: 600 },
    dx: 10,
    dy: 20,
  });
  const at = (id) => {
    const i = [...out.nodes, ...out.notes, ...out.groups].find(
      (n) => n.id === id,
    );
    return [i.x, i.y];
  };
  assert.deepEqual(at("outer"), [10, 20]);
  assert.deepEqual(at("inner"), [410, 120]);
  assert.deepEqual(at("in"), [30, 60]);
  assert.deepEqual(at("deep"), [430, 160]);
  assert.deepEqual(at("f"), [30, 320]);
  assert.deepEqual(at("n"), [30, 520]);
  assert.deepEqual(at("away"), [1200, 40]);
  const back = JSON.parse(serializeDiagram(out)).nodes.find(
    (n) => n.id === "inner",
  );
  assert.deepEqual([back.x, back.y], [410, 120]);
});

test("a picked group carries what lies inside it, which then moves no other way", async () => {
  const { parseDiagram } = await model;
  const { moveSet } = await board;
  const d = parseDiagram(
    file([
      grp("g", 0, 0, 800, 600),
      grp("h", 400, 100, 300, 200),
      textNode("a", { x: 20, y: 40 }),
      textNode("n", {
        x: 20,
        y: 500,
        width: 280,
        height: 30,
        sushiai: { note: true },
      }),
      textNode("c", { x: 1000, y: 40 }),
    ]),
  ).diagram;
  const picks = ["g", "h"]
    .map((id) => ({ type: "group", id }))
    .concat([
      { type: "node", id: "a" },
      { type: "note", id: "n" },
      { type: "node", id: "c" },
    ]);
  assert.deepEqual(moveSet(d, picks), {
    nodes: ["c"],
    notes: [],
    groups: ["g"],
  });
});

test("setting arrow end on a start-only edge is written", async () => {
  const { parseDiagram, applyOp, serializeDiagram } = await model;
  const d = parseDiagram(
    file(
      [textNode("a"), textNode("b", { x: 300 })],
      [
        {
          id: "e",
          fromNode: "a",
          toNode: "b",
          fromEnd: "arrow",
          toEnd: "none",
        },
      ],
    ),
  ).diagram;
  const edge = (dd) => JSON.parse(serializeDiagram(dd)).edges[0];
  assert.deepEqual([edge(d).fromEnd, edge(d).toEnd], ["arrow", "none"]);
  const end = applyOp(d, { op: "set", id: "e", field: "arrow", value: "end" });
  assert.deepEqual(
    [edge(end).fromEnd, edge(end).toEnd],
    [undefined, undefined],
  );
});

test("set group leaves a block outside every group, or in the group asked for", async () => {
  const { parseDiagram, applyOp, groupRects, groupAt, nodeSize } = await model;
  const d = parseDiagram(
    file([
      grp("a", 0, 0, 300, 200),
      grp("b", 320, 0, 300, 200),
      grp("big", 0, 400, 800, 600),
      grp("small", 100, 500, 300, 200),
      textNode("x", { x: 20, y: 40 }),
      textNode("y", { x: 20, y: 800 }),
    ]),
  ).diagram;
  const centreGroup = (dd, id) => {
    const n = dd.nodes.find((i) => i.id === id);
    const { w, h } = nodeSize(n);
    return groupAt(groupRects(dd), { x: n.x + w / 2, y: n.y + h / 2 });
  };
  const out = applyOp(d, { op: "set", id: "x", field: "group", value: "" });
  assert.equal(centreGroup(out, "x"), "");
  assert.equal(out.nodes[0].group, undefined);
  // the group beside it is not a landing place either
  const wide = applyOp(
    parseDiagram(
      file([
        grp("a", 0, 0, 300, 200),
        grp("b", 0, 210, 300, 200),
        textNode("x", { x: 20, y: 40 }),
      ]),
    ).diagram,
    { op: "set", id: "x", field: "group", value: "" },
  );
  assert.equal(centreGroup(wide, "x"), "");
  // into "big" while its centre is inside "small": the block leaves "small"
  const into = applyOp(d, {
    op: "set",
    id: "y",
    field: "group",
    value: "small",
  });
  assert.equal(centreGroup(into, "y"), "small");
  const bigger = applyOp(
    parseDiagram(
      file([
        grp("big", 0, 400, 800, 600),
        grp("small", 100, 500, 300, 200),
        textNode("y", { x: 120, y: 520 }),
      ]),
    ).diagram,
    { op: "set", id: "y", field: "group", value: "big" },
  );
  assert.equal(centreGroup(bigger, "y"), "big");
});

test("a note keeps its colour, and a link must join node ids", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  const note = textNode("n", {
    color: "#112233",
    width: 280,
    height: 30,
    sushiai: { note: true },
  });
  const out = nodesOf(serializeDiagram(parseDiagram(file([note])).diagram));
  assert.equal(out[0].color, "#112233");
  const plain = { ...note, color: undefined };
  assert.equal(
    nodesOf(serializeDiagram(parseDiagram(file([plain])).diagram))[0].color,
    "3",
  );
  const bad = parseDiagram(
    file(
      [textNode("a"), textNode("b", { x: 300 })],
      [
        { id: "e1", fromNode: "a", toNode: "b" },
        { id: "e2", fromNode: "a", toNode: "e1" },
      ],
    ),
  );
  assert.match(bad.error, /edges\[1\] \(e2\).*toNode references unknown node/);
});

test("a file or link node refuses title, tech and caption edits", async () => {
  const { parseDiagram, applyOp } = await model;
  const d = parseDiagram(
    file([
      {
        id: "f",
        type: "file",
        file: "a/b.md",
        x: 0,
        y: 0,
        width: 4,
        height: 4,
      },
    ]),
  ).diagram;
  for (const field of ["title", "tech", "caption"]) {
    assert.equal(
      applyOp(d, { op: "set", id: "f", field, value: "X" }),
      d,
      field,
    );
  }
});

const LONG =
  "A very long title for one block that has to wrap onto several lines";
const SUB =
  "A subtitle with plenty of words so that it needs to wrap onto a third line too";

test("a block grows to its text and wraps beyond the maximum width", async () => {
  const { nodeSize } = await model;
  const short = nodeSize({ title: "API" });
  assert.deepEqual(short, { w: 170, h: 62 });
  const wide = nodeSize({ title: "A title of medium length", shape: "box" });
  assert.ok(wide.w >= short.w && wide.w <= 320);
  const long = nodeSize({ title: LONG, tech: SUB });
  assert.equal(long.w, 320);
  assert.ok(long.h > short.h);
  assert.ok(long.h >= 14 + 16 * 2 + 13 * 2);
  const more = nodeSize({ title: LONG + LONG, tech: SUB + SUB });
  assert.ok(more.h > long.h, "more text, more height");
  assert.equal(more.w, 320);
});

test("a block sizes itself unless its size was chosen", async () => {
  const { parseDiagram, nodeSize, MIN_NODE_W } = await model;
  const read = (extra, sushiai) =>
    parseDiagram(
      file([
        textNode("a", {
          text: `${LONG}\n${SUB}`,
          ...extra,
          ...(sushiai && { sushiai }),
        }),
      ]),
    ).diagram.nodes[0];
  const auto = nodeSize(read({ width: 170, height: 62 }));
  assert.equal(auto.w, 320, "the file's width alone fixes nothing");
  assert.deepEqual(nodeSize(read({ width: 99, height: 9 })), auto);
  const chosen = { size: "fixed" };
  const narrow = nodeSize(read({ width: 120, height: 62 }, chosen));
  assert.equal(narrow.w, 120, "a chosen width is kept, narrower or not");
  assert.ok(narrow.h > auto.h, "narrower wraps onto more lines");
  assert.equal(nodeSize(read({ width: 1, height: 1 }, chosen)).w, MIN_NODE_W);
  assert.deepEqual(nodeSize(read({ width: 400, height: 300 }, chosen)), {
    w: 400,
    h: 300,
  });
  assert.equal(read({ width: 120, height: 62 }, chosen).w, 120);
});

test("a long word breaks at the width and the next word starts after its tail", async () => {
  const { lineCount } = await measure;
  const word = "x".repeat(40);
  assert.equal(lineCount(`${word} bbbbbbbbbbb`, 12.5, 100, true), 4);
  assert.equal(lineCount("ab cd", 12.5, 100, true), 1);
  assert.equal(lineCount("a\n\nb", 12.5, 100, false, true), 3);
});

test("a block with a kind row, a title and a subtitle is as tall as its CSS", async () => {
  const { nodeSize } = await model;
  // 8 + 7 padding, 2 border, 13 row + 3 gap, 16 title, 3 gap, 13 subtitle.
  const size = nodeSize({ title: "T", tech: "t", caption: "Step" });
  assert.equal(size.h, 66);
  const wide = nodeSize({
    title: "T",
    caption: "A caption that is quite long indeed",
  });
  assert.ok(wide.w > 170, "the caption row has room");
});

test("resize-node stores the size, round-trips it and keeps the text whole", async () => {
  const {
    parseDiagram,
    applyOp,
    serializeDiagram,
    nodeSize,
    MIN_NODE_W,
    MIN_NODE_H,
  } = await model;
  const d = parseDiagram(file([textNode("a", { text: "A" })])).diagram;
  const out = applyOp(d, { op: "resize-node", id: "a", w: 300, h: 140 });
  assert.deepEqual(nodeSize(out.nodes[0]), { w: 300, h: 140 });
  const saved = JSON.parse(serializeDiagram(out)).nodes[0];
  assert.deepEqual([saved.width, saved.height], [300, 140]);
  const again = parseDiagram(serializeDiagram(out)).diagram;
  assert.deepEqual(nodeSize(again.nodes[0]), { w: 300, h: 140 });
  assert.equal(serializeDiagram(again), serializeDiagram(out));
  const small = applyOp(out, { op: "resize-node", id: "a", w: 10, h: 10 });
  assert.equal(nodeSize(small.nodes[0]).w, MIN_NODE_W);
  assert.ok(nodeSize(small.nodes[0]).h >= MIN_NODE_H);
  const long = applyOp(
    { ...d, nodes: [{ ...d.nodes[0], title: LONG }] },
    { op: "resize-node", id: "a", w: 10, h: 10 },
  );
  assert.ok(nodeSize(long.nodes[0]).h > MIN_NODE_H, "the text still fits");
  assert.equal(applyOp(d, { op: "resize-node", id: "zz", w: 9, h: 9 }), d);
  assert.equal(applyOp(d, { op: "resize-node", id: "a", w: NaN, h: 9 }), d);
});

test("a block nobody touched keeps the file's width and height on save", async () => {
  const { parseDiagram, applyOp, serializeDiagram, nodeSize } = await model;
  const text = file([
    textNode("a", { text: `${LONG}\n${SUB}`, width: 170, height: 62 }),
    textNode("b", { x: 400 }),
  ]);
  const d = parseDiagram(text).diagram;
  const moved = applyOp(d, { op: "move", id: "b", x: 500, y: 0 });
  const a = JSON.parse(serializeDiagram(moved)).nodes[0];
  assert.deepEqual([a.width, a.height], [170, 62]);
  const sized = applyOp(moved, { op: "resize-node", id: "a", w: 170, h: 62 });
  const grown = JSON.parse(serializeDiagram(sized)).nodes[0];
  assert.equal(grown.width, 170);
  assert.ok(grown.height > 62, "after a resize the drawn size is written");
  assert.equal(grown.sushiai.size, "fixed");
  const back = parseDiagram(serializeDiagram(sized)).diagram;
  assert.equal(back.nodes[0].w, 170);
  assert.equal(serializeDiagram(back), serializeDiagram(sized));
  const reshaped = applyOp(sized, {
    op: "set",
    id: "a",
    field: "shape",
    value: "pill",
  });
  assert.equal(reshaped.nodes[0].w, undefined, "a new shape clears the size");
  const edited = applyOp(d, { op: "set", id: "a", field: "title", value: "T" });
  const written = JSON.parse(serializeDiagram(edited)).nodes[0];
  const drawn = nodeSize(edited.nodes[0]);
  assert.deepEqual([written.width, written.height], [drawn.w, drawn.h]);
  assert.notEqual(written.width, 170, "an edited block says what it draws");
  assert.equal(written.sushiai?.size, undefined);
});

test("a note holds several lines and its size follows them", async () => {
  const { parseDiagram, applyOp, serializeDiagram, noteSize } = await model;
  const note = (text) => ({ id: "n", text, x: 0, y: 0 });
  const one = noteSize(note("Ready?"));
  const three = noteSize(note("First\nSecond\nThird"));
  assert.ok(three.h >= one.h + 2 * 15);
  const wrapped = noteSize(note("word ".repeat(60)));
  assert.equal(wrapped.w, 280);
  assert.ok(wrapped.h > three.h);
  const d = parseDiagram(
    file([
      {
        id: "n",
        type: "text",
        text: "A\nB",
        x: 5,
        y: 6,
        width: 280,
        height: 30,
        color: "3",
        sushiai: { note: true },
      },
    ]),
  ).diagram;
  assert.equal(d.notes[0].text, "A\nB");
  const edited = applyOp(d, {
    op: "set",
    id: "n",
    field: "text",
    value: "x\ny\nz",
  });
  const saved = JSON.parse(serializeDiagram(edited)).nodes[0];
  assert.equal(saved.text, "x\ny\nz");
  assert.deepEqual(
    [saved.width, saved.height],
    [noteSize(edited.notes[0]).w, noteSize(edited.notes[0]).h],
  );
  assert.deepEqual(
    parseDiagram(serializeDiagram(edited)).diagram.notes,
    edited.notes,
  );
});

test("an arrow at the start only is a direction of its own and round-trips", async () => {
  const { parseDiagram, applyOp, serializeDiagram } = await model;
  const link = (extra) => ({ id: "e", fromNode: "a", toNode: "b", ...extra });
  const read = (extra) =>
    parseDiagram(
      file([textNode("a"), textNode("b", { x: 300 })], [link(extra)]),
    ).diagram;
  const d = read({ fromEnd: "arrow", toEnd: "none" });
  assert.equal(d.edges[0].arrow, "start");
  assert.equal(d.edges[0]._jc, undefined);
  const saved = JSON.parse(serializeDiagram(d)).edges[0];
  assert.deepEqual([saved.fromEnd, saved.toEnd], ["arrow", "none"]);
  assert.equal(read({}).edges[0].arrow, undefined);
  assert.equal(read({ toEnd: "none" }).edges[0].arrow, "none");
  const set = applyOp(read({}), {
    op: "set",
    id: "e",
    field: "arrow",
    value: "start",
  });
  assert.equal(set.edges[0].arrow, "start");
});

const chip = (spot, text) => {
  const w = text.length * 6 + 10;
  return spot.vertical
    ? { x: spot.flip ? spot.x - 6 - w : spot.x + 6, y: spot.y - 8, w, h: 16 }
    : { x: spot.x - w / 2, y: spot.flip ? spot.y + 6 : spot.y - 22, w, h: 16 };
};
const apart = (a, b) =>
  a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;

test("labels of two edges between the same blocks never overlap each other or a head", async () => {
  const { placeLabels } = await model;
  const routes = [
    {
      id: "ab",
      text: "request",
      points: [
        { x: 170, y: 22 },
        { x: 300, y: 22 },
      ],
    },
    {
      id: "ba",
      text: "response",
      points: [
        { x: 300, y: 38 },
        { x: 170, y: 38 },
      ],
    },
  ];
  const spots = placeLabels(routes, []);
  const a = chip(spots.get("ab"), "request");
  const b = chip(spots.get("ba"), "response");
  assert.ok(apart(a, b), "chips apart");
  for (const head of [
    { x: 290, y: 12, w: 20, h: 20 },
    { x: 160, y: 28, w: 20, h: 20 },
  ]) {
    assert.ok(apart(a, head) && apart(b, head), "clear of the arrowheads");
  }
  for (const line of [22, 38]) {
    for (const c of [a, b]) {
      assert.ok(
        c.y + c.h <= line || c.y >= line,
        "chip does not sit on a line",
      );
    }
  }
});

test("a label slides off a block and off the end segment's arrowhead", async () => {
  const { placeLabels } = await model;
  const points = [
    { x: 0, y: 100 },
    { x: 200, y: 100 },
  ];
  const block = { x: 60, y: 60, w: 80, h: 30 };
  const spot = placeLabels([{ id: "e", text: "go", points }], [block]).get("e");
  const c = chip(spot, "go");
  assert.ok(apart(c, block));
  assert.ok(apart(c, { x: 190, y: 90, w: 20, h: 20 }));
});

const pairBetween = (gap) => {
  const left = { x: 0, y: 0, w: 170, h: 62 };
  const right = { x: 170 + gap, y: 0, w: 170, h: 62 };
  const routes = [
    {
      id: "ab",
      text: "cache lookup",
      points: [
        { x: 170, y: 22 },
        { x: 170 + gap, y: 22 },
      ],
    },
    {
      id: "ba",
      text: "cached session",
      points: [
        { x: 170 + gap, y: 38 },
        { x: 170, y: 38 },
      ],
    },
  ];
  return { blocks: [left, right], routes };
};

test("antiparallel labels between blocks 100 px apart touch no block, one out each side", async () => {
  const { placeLabels } = await model;
  const { blocks, routes } = pairBetween(100);
  const spots = placeLabels(routes, blocks);
  const a = chip(spots.get("ab"), "cache lookup");
  const b = chip(spots.get("ba"), "cached session");
  for (const c of [a, b]) {
    for (const block of blocks) assert.ok(apart(c, block), "clear of blocks");
  }
  assert.ok(apart(a, b));
  assert.ok(a.y + a.h <= 22, "the top label is above the top line");
  assert.ok(b.y >= 38, "the bottom label is below the bottom line");
});

test("when a gap is too narrow a label stays centred on it, not half under a block", async () => {
  const { placeLabels } = await model;
  const { blocks, routes } = pairBetween(60);
  const spots = placeLabels(routes, blocks);
  for (const id of ["ab", "ba"]) {
    assert.equal(spots.get(id).x, 200, `${id} sits on the middle of the run`);
  }
  const a = chip(spots.get("ab"), "cache lookup");
  const b = chip(spots.get("ba"), "cached session");
  assert.ok(apart(a, b));
});

test("a note keeps the file's width and height through an unrelated edit, until its text is edited", async () => {
  const { parseDiagram, applyOps, serializeDiagram, noteSize } = await model;
  const note = {
    id: "n",
    type: "text",
    text: "note\ntwo",
    x: 5,
    y: 6,
    width: 280,
    height: 30,
    color: "3",
    sushiai: { note: true },
  };
  const text = file([note, textNode("b", { x: 400 })]);
  const d = parseDiagram(text).diagram;
  const moved = applyOps(d, [{ op: "move", id: "b", x: 500, y: 0 }]);
  const saved = JSON.parse(serializeDiagram(moved)).nodes[0];
  assert.deepEqual(saved, note);
  assert.ok(noteSize(d.notes[0]).h !== 30, "it still draws the fitted size");
  const edited = applyOps(d, [
    { op: "set", id: "n", field: "text", value: "a\nb\nc" },
  ]);
  const out = JSON.parse(serializeDiagram(edited)).nodes[0];
  const fit = noteSize(edited.notes[0]);
  assert.deepEqual([out.width, out.height], [fit.w, fit.h]);
});
