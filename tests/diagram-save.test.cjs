const { test } = require("node:test");
const assert = require("node:assert/strict");

const { until } = require("./helpers/deadline.cjs");

const save = import("../src/extensions/preview/diagram/saveOps.ts");

const text = (id, label, x, own = {}) => ({
  id,
  type: "text",
  text: label,
  x,
  y: 0,
  width: 170,
  height: 62,
  sushiai: own,
});
const file = (extra = []) =>
  JSON.stringify({
    nodes: [
      text("api", "API", 0, { kind: "service" }),
      text("db", "DB", 300, { kind: "data" }),
      ...extra,
    ],
    edges: [],
  });

// A file on "disk" with a hash per version; `hooks.beforeWrite` lets a test
// change it the way an agent would between the owner's read and write.
function disk(text, hooks = {}) {
  const state = { text, version: 1, writes: [], reads: 0 };
  const hashOf = () => `h${state.version}`;
  return {
    state,
    io: {
      read: async () => {
        state.reads += 1;
        return { text: state.text, hash: hashOf() };
      },
      write: async (body, expected) => {
        hooks.beforeWrite?.(state);
        if (expected !== hashOf())
          throw new Error("File changed on disk. Reload it before saving.");
        state.text = body;
        state.version += 1;
        state.writes.push(body);
        return { hash: hashOf() };
      },
    },
  };
}

const move = { op: "move", id: "api", x: 40, y: 20 };

test("a conflict is read again and the second write lands", async () => {
  const { saveOps } = await save;
  let hit = false;
  const d = disk(file(), {
    beforeWrite: (s) => {
      if (hit) return;
      hit = true;
      s.version += 1;
    },
  });
  const done = await saveOps(d.io, [move]);
  assert.equal(d.state.reads, 2);
  assert.equal(d.state.writes.length, 1);
  assert.equal(JSON.parse(d.state.text).nodes[0].x, 40);
  assert.equal(done.hash, `h${d.state.version}`);
});

test("three conflicts in a row are an error", async () => {
  const { saveOps } = await save;
  const d = disk(file(), { beforeWrite: (s) => (s.version += 1) });
  await assert.rejects(saveOps(d.io, [move]), /changed on disk/i);
  assert.equal(d.state.reads, 3);
  assert.equal(d.state.writes.length, 0);
});

test("an unparseable file is an error and nothing is written", async () => {
  const { saveOps } = await save;
  const d = disk("{ not json");
  await assert.rejects(saveOps(d.io, [move]));
  assert.equal(d.state.writes.length, 0);
});

test("another error is not retried", async () => {
  const { saveOps } = await save;
  let writes = 0;
  const io = {
    read: async () => ({ text: file(), hash: "a" }),
    write: async () => {
      writes += 1;
      throw new Error("Permission denied");
    },
  };
  await assert.rejects(saveOps(io, [move]), /Permission denied/);
  assert.equal(writes, 1);
});

test("a node the agent adds between read and write survives", async () => {
  const { saveOps } = await save;
  const agent = text("cache", "Cache", 600, { kind: "data" });
  let done = false;
  const d = disk(file(), {
    beforeWrite: (s) => {
      if (done) return;
      done = true;
      s.text = file([agent]);
      s.version += 1;
    },
  });
  await saveOps(d.io, [move]);
  const nodes = JSON.parse(d.state.text).nodes;
  assert.ok(nodes.some((n) => n.id === "cache"));
  assert.equal(nodes.find((n) => n.id === "api").x, 40);
});

test("an owner node whose id the agent took meanwhile keeps both", async () => {
  const { saveOps } = await save;
  const mine = {
    op: "add-node",
    node: { id: "component", kind: "service", title: "Mine" },
  };
  const d = disk(file([text("component", "Agent's", 600)]));
  await saveOps(d.io, [mine]);
  const titles = JSON.parse(d.state.text).nodes.map((n) => n.text);
  assert.ok(titles.includes("Mine") && titles.includes("Agent's"));
});

test("an op pushed during a write goes out in a second write, nothing dropped", async () => {
  const { createSaveQueue } = await save;
  const d = disk(file());
  let release;
  let firstWrite = true;
  const gate = new Promise((resolve) => (release = resolve));
  const io = {
    read: d.io.read,
    write: async (body, expected) => {
      if (firstWrite) {
        firstWrite = false;
        await gate;
      }
      return d.io.write(body, expected);
    },
  };
  const events = [];
  const queue = createSaveQueue(() => io, {
    pending: (ops) => events.push(ops.length),
    saving: () => {},
    saved: () => {},
    idle: () => events.push("idle"),
    failed: () => events.push("failed"),
  });
  queue.push(move);
  const run = queue.flush();
  await until(() => !firstWrite, "the first write to start");
  queue.push({ op: "move", id: "db", x: 320, y: 40 });
  // A flush while one runs does nothing; the running loop drains the rest.
  await queue.flush();
  release();
  await run;
  const nodes = JSON.parse(d.state.text).nodes;
  assert.equal(d.state.writes.length, 2);
  assert.deepEqual([nodes[0].x, nodes[1].x, nodes[1].y], [40, 320, 40]);
  assert.equal(queue.pending().length, 0);
  assert.equal(events.at(-1), "idle");
});

test("a failed save keeps its ops and a later flush writes them", async () => {
  const { createSaveQueue } = await save;
  const d = disk(file());
  let broken = true;
  const io = {
    read: d.io.read,
    write: async (body, expected) => {
      if (broken) throw new Error("Permission denied");
      return d.io.write(body, expected);
    },
  };
  const seen = [];
  const queue = createSaveQueue(() => io, {
    pending: () => {},
    saving: () => {},
    saved: () => seen.push("saved"),
    idle: () => seen.push("idle"),
    failed: () => seen.push("failed"),
  });
  queue.push(move);
  await queue.flush();
  assert.equal(queue.pending().length, 1);
  assert.equal(queue.hasFailed(), true);
  assert.equal(d.state.writes.length, 0);
  broken = false;
  await queue.flush();
  assert.equal(queue.pending().length, 0);
  assert.equal(queue.hasFailed(), false);
  assert.equal(JSON.parse(d.state.text).nodes[0].x, 40);
  assert.deepEqual(seen, ["failed", "saved", "idle"]);
});

test("five owner saves send one notice; an agent write re-arms one more", async () => {
  const { createEditNotice } = await save;
  const notice = createEditNotice("h1");
  const sent = [];
  let hash = "h1";
  const ownerSave = (next) => {
    notice.saving(hash);
    sent.push(notice.saved(next, hash));
    hash = next;
    notice.polled(hash);
  };
  for (let i = 2; i <= 6; i++) ownerSave(`h${i}`);
  assert.deepEqual(sent, [true, false, false, false, false]);
  hash = "agent-1";
  notice.polled(hash);
  ownerSave("h7");
  ownerSave("h8");
  assert.deepEqual(sent.slice(5), [true, false]);
});

test("an agent write overwritten before the next poll still re-arms the notice", async () => {
  const { createEditNotice } = await save;
  const notice = createEditNotice("h0");
  notice.saving("h0");
  assert.equal(notice.saved("h1", "h0"), true);
  notice.polled("h1");
  notice.saving("h1");
  assert.equal(notice.saved("h3", "h2"), true);
  notice.polled("h3");
  notice.saving("h3");
  assert.equal(notice.saved("h4", "h3"), false);
});

test("a poll that sees an owner write before its reply arrives sends no second notice", async () => {
  const { createEditNotice } = await save;
  const notice = createEditNotice("h0");
  const sent = [];
  let hash = "h0";
  for (let i = 1; i <= 5; i++) {
    notice.saving(hash);
    notice.polled(`h${i}`);
    sent.push(notice.saved(`h${i}`, hash));
    hash = `h${i}`;
  }
  assert.deepEqual(sent, [true, false, false, false, false]);
});

test("a write by the agent polled during an owner save re-arms the notice", async () => {
  const { createEditNotice } = await save;
  const notice = createEditNotice("h0");
  notice.saving("h0");
  assert.equal(notice.saved("h1", "h0"), true);
  notice.saving("h1");
  notice.polled("agent-1");
  assert.equal(notice.saved("h2", "h1"), true);
});

test("an agent write polled after a failed owner save re-arms the notice at once", async () => {
  const { createEditNotice } = await save;
  const notice = createEditNotice("h0");
  notice.saving("h0");
  assert.equal(notice.saved("h1", "h0"), true);
  notice.saving("h1");
  notice.failed();
  notice.polled("agent-1");
  notice.saving("agent-1");
  assert.equal(notice.saved("h2", "agent-1"), true);
});

test("saveOps returns the hash it read", async () => {
  const { saveOps } = await save;
  const d = disk(file());
  const done = await saveOps(d.io, [move]);
  assert.equal(done.readHash, "h1");
  assert.equal(done.hash, "h2");
});

test("a rename from one batch reaches ops queued behind it", async () => {
  const { createSaveQueue } = await save;
  const agentGroup = {
    id: "boundary",
    type: "group",
    label: "Agent",
    x: 0,
    y: 200,
    width: 300,
    height: 150,
  };
  const d = disk(JSON.stringify({ nodes: [agentGroup], edges: [] }));
  const maps = [];
  const queue = createSaveQueue(() => d.io, {
    pending: () => {},
    saving: () => {},
    saved: () => {},
    remapped: (map) => maps.push(Object.fromEntries(map)),
    idle: () => {},
    failed: (error) => assert.fail(String(error)),
  });
  const mine = { id: "boundary", label: "Mine", x: 400, y: 0, w: 300, h: 150 };
  queue.push({ op: "add-group", group: mine });
  const first = queue.flush();
  queue.push({ op: "set", id: "boundary", field: "label", value: "Renamed" });
  await first;
  const groups = JSON.parse(d.state.text).nodes.filter(
    (n) => n.type === "group",
  );
  assert.deepEqual(
    groups.map((g) => [g.id, g.label]),
    [
      ["boundary", "Agent"],
      ["boundary-2", "Renamed"],
    ],
  );
  assert.deepEqual(maps, [{ boundary: "boundary-2" }]);
});
