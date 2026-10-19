const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/extensions/preview/diagram/model.ts");

const base = () => ({
  version: 1,
  groups: [{ id: "cloud", label: "Cloud" }],
  nodes: [
    { id: "api", kind: "service", title: "API", group: "cloud" },
    { id: "db", kind: "data", title: "DB", tech: "pg", delta: "new" },
  ],
  edges: [{ id: "e1", from: "api", to: "db", label: "sql" }],
  notes: [{ id: "n1", text: "why?", x: 5, y: 6 }],
});

test("parseDiagram accepts a file and defaults missing arrays", async () => {
  const { parseDiagram } = await model;
  const r = parseDiagram('{"version":1}');
  assert.deepEqual(r.diagram, {
    version: 1,
    groups: [],
    nodes: [],
    edges: [],
    notes: [],
  });
  assert.ok(parseDiagram(JSON.stringify(base())).diagram);
});

test("parseDiagram rejects bad files with an error naming the item", async () => {
  const { parseDiagram } = await model;
  const bad = (mut) => {
    const d = base();
    mut(d);
    return parseDiagram(JSON.stringify(d)).error;
  };
  assert.match(parseDiagram("{").error, /not valid JSON/);
  assert.match(parseDiagram("[]").error, /object/);
  assert.match(parseDiagram('{"version":2}').error, /version/);
  assert.match(parseDiagram('{"version":1,"nodes":{}}').error, /nodes/);
  assert.equal(
    bad((d) => (d.nodes[1].kind = "db")),
    'nodes[1] (db): unknown kind "db"',
  );
  assert.match(
    bad((d) => (d.nodes[1].id = "api")),
    /nodes\[1\] \(api\): duplicate id/,
  );
  assert.match(
    bad((d) => (d.nodes[0].id = "e1")),
    /duplicate id/,
  );
  assert.match(
    bad((d) => (d.nodes[0].id = "")),
    /id must be a non-empty/,
  );
  assert.match(
    bad((d) => (d.edges[0].to = "ghost")),
    /edges\[0\] \(e1\).*ghost/,
  );
  assert.match(
    bad((d) => (d.nodes[0].group = "nope")),
    /unknown group/,
  );
  assert.match(
    bad((d) => (d.nodes[0].delta = "old")),
    /unknown delta/,
  );
  assert.match(
    bad((d) => (d.nodes[0].status = "done")),
    /unknown status/,
  );
  assert.match(
    bad((d) => (d.edges[0].style = "wavy")),
    /unknown style/,
  );
  assert.match(
    bad((d) => (d.nodes[0].x = "1")),
    /x must be a finite/,
  );
  assert.match(
    bad((d) => delete d.notes[0].y),
    /notes\[0\] \(n1\)/,
  );
  assert.match(
    bad((d) => delete d.notes[0].text),
    /text/,
  );
});

test("unknown fields survive parse and serialize", async () => {
  const { parseDiagram, serializeDiagram } = await model;
  const d = base();
  d.future = { a: 1 };
  d.nodes[0].owner = "team";
  d.edges[0].weight = 3;
  const text = JSON.stringify(d, null, 2) + "\n";
  const r = parseDiagram(text);
  assert.equal(serializeDiagram(r.diagram), text);
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

test("placeNodes puts group members in explicit bounds or beside members", async () => {
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

  const e = {
    version: 1,
    groups: [{ id: "g", label: "G" }],
    nodes: [nd("a", { group: "g", x: 300, y: 300 }), nd("b", { group: "g" })],
    edges: [],
    notes: [],
  };
  const p = placeNodes(e).get("b");
  assert.deepEqual(p, { x: 510, y: 300 });
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

test("groupRects pads members and unions explicit bounds", async () => {
  const { groupRects } = await model;
  const d = {
    version: 1,
    groups: [
      { id: "g", label: "G" },
      { id: "e", label: "E" },
      { id: "x", label: "X", x: 0, y: 0, w: 100, h: 100 },
    ],
    nodes: [
      nd("a", { group: "g" }),
      nd("b", { group: "g" }),
      nd("c", { group: "x" }),
    ],
    edges: [],
    notes: [],
  };
  const pos = new Map([
    ["a", { x: 100, y: 100 }],
    ["b", { x: 400, y: 200 }],
    ["c", { x: 50, y: 50 }],
  ]);
  const r = groupRects(d, pos);
  assert.deepEqual(r.get("g"), {
    x: 90,
    y: 70,
    w: 490,
    h: 262 - 70 + 8,
  });
  assert.equal(r.has("e"), false);
  assert.deepEqual(r.get("x"), { x: 0, y: 0, w: 230, h: 120 });
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
  const nodes = Array.from({ length: 1001 }, (_, i) => nd(`n${i}`));
  assert.match(parseDiagram(JSON.stringify(bare(nodes))).error, /1000/);
  assert.ok(parseDiagram(JSON.stringify(bare(nodes.slice(1)))).diagram);
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
  const { parseDiagram, groupRects, placeNodes } = await model;
  const text = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "fixtures/shop.arch.json"),
    "utf8",
  );
  const { diagram } = parseDiagram(text);
  assert.equal(diagram.nodes.length, 9);
  assert.equal(diagram.edges.length, 8);
  assert.equal(diagram.groups.length, 4);
  assert.equal(diagram.notes.length, 1);
  const rects = groupRects(diagram, placeNodes(diagram));
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
    require("node:path").join(__dirname, "fixtures/shop.arch.json"),
    "utf8",
  );
  const { diagram } = parseDiagram(text);
  const pos = placeNodes(diagram);
  const rect = (id) => ({ ...pos.get(id), w: 170, h: 62 });
  const labels = diagram.groups.map((g) =>
    groupLabelRect(groupRects(diagram, pos).get(g.id), g.label),
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
  const doc = (n) => JSON.stringify(bare([n]));
  assert.match(
    parseDiagram(doc(nd("a", { x: 1e20, y: 0 }))).error,
    /nodes\[0\] \(a\)/,
  );
  assert.ok(parseDiagram(doc(nd("a", { x: 1e5, y: -1e5 }))).diagram);
  const g = (extra) =>
    JSON.stringify({
      ...bare([]),
      groups: [{ id: "g", label: "G", ...extra }],
    });
  assert.match(
    parseDiagram(g({ x: 0, y: 0, w: 0, h: 5 })).error,
    /groups\[0\]/,
  );
  assert.match(
    parseDiagram(g({ x: 0, y: 0, w: 5, h: 1e9 })).error,
    /groups\[0\]/,
  );
  const note = { ...bare([]), notes: [{ id: "n", text: "t", x: 1e9, y: 0 }] };
  assert.match(parseDiagram(JSON.stringify(note)).error, /notes\[0\]/);

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
  d.nodes.push({ id: "other", kind: "app", title: "O", x: 1, y: 1 });
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
  assert.deepEqual([out.nodes[2].x, out.nodes[2].y], [1, 1]);
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
    { op: "add-group", group: { id: "cloud", label: "Mine" } },
    { op: "add-note", note: { id: "n1", text: "mine", x: 0, y: 0 } },
    { op: "set", id: "api", field: "group", value: "cloud" },
    {
      op: "move-group",
      id: "cloud",
      rect: { x: 0, y: 0, w: 300, h: 200 },
      dx: 0,
      dy: 0,
    },
    { op: "resize-group", id: "cloud", rect: { x: 0, y: 0, w: 400, h: 300 } },
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
    x: 0,
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
  const withNode = (extra, edge = {}) =>
    parseDiagram(
      JSON.stringify({
        version: 1,
        nodes: [
          { id: "a", title: "A", ...extra },
          { id: "b", title: "B" },
        ],
        edges: [{ id: "e", from: "a", to: "b", ...edge }],
      }),
    );
  assert.ok(withNode({}).diagram, "id and title are enough");
  assert.ok(
    withNode(
      {
        shape: "diamond",
        icon: "bot",
        caption: "Agent",
        tech: "t",
        step: 3,
        delta: "removed",
        kind: "app",
      },
      { style: "dashed", arrow: "both" },
    ).diagram,
  );
  const err = (extra, edge) => withNode(extra, edge).error;
  assert.match(err({ shape: "star" }), /unknown shape/);
  assert.match(err({ icon: 5 }), /icon/);
  assert.match(err({ icon: "x".repeat(41) }), /icon/);
  assert.match(err({ caption: 5 }), /caption/);
  assert.match(err({ tech: 5 }), /tech/);
  assert.ok(
    withNode({ caption: "x".repeat(500), tech: "y".repeat(500) }).diagram,
  );
  assert.ok(withNode({ icon: "not-in-the-table" }).diagram);
  for (const step of [0, 1.5, 1000, "2", -1]) {
    assert.match(err({ step }), /step/);
  }
  assert.match(err({ kind: "db" }), /unknown kind/);
  assert.match(err({ delta: "old" }), /unknown delta/);
  assert.match(err({}, { arrow: "up" }), /unknown arrow/);
  assert.match(err({}, { style: "wavy" }), /unknown style/);
  const g = parseDiagram(
    '{"version":1,"groups":[{"id":"g","label":"G","icon":"lock"}]}',
  );
  assert.ok(g.diagram);
  assert.ok(
    parseDiagram('{"version":1,"groups":[{"id":"g","label":"G","icon":"x"}]}')
      .diagram,
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
  const nodes = PRESETS.map((p) => ({ id: p.id, title: p.label, ...p.node }));
  assert.ok(parseDiagram(JSON.stringify({ version: 1, nodes })).diagram);
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

test("a group wraps members by their own sizes", async () => {
  const { groupRects } = await model;
  const d = {
    version: 1,
    groups: [{ id: "g", label: "G" }],
    notes: [],
    edges: [],
    nodes: [
      { id: "a", title: "A", shape: "diamond", group: "g" },
      { id: "b", title: "B", shape: "circle", group: "g" },
    ],
  };
  const pos = new Map([
    ["a", { x: 100, y: 100 }],
    ["b", { x: 400, y: 100 }],
  ]);
  assert.deepEqual(groupRects(d, pos).get("g"), {
    x: 90,
    y: 70,
    w: 72 + 400 - 100 + 20,
    h: 90 + 30 + 8,
  });
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
    require("node:path").join(__dirname, "fixtures/agent-flow.arch.json"),
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
    require("node:path").join(__dirname, "fixtures/shop.arch.json"),
    "utf8",
  );
  const d = parseDiagram(text).diagram;
  const pos = placeNodes(d);
  const avoid = [...groupRects(d, pos).values()];
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
  check("agent-flow", read("agent-flow.arch.json"));
  check("shop", read("shop.arch.json"));
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
  for (const d of [
    read("agent-flow.arch.json"),
    read("shop.arch.json"),
    mixed,
  ]) {
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
  for (const name of ["agent-flow.arch.json", "shop.arch.json"]) {
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
  const { d, rects, routes } = await fixtureRoutes("agent-flow.arch.json");
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
