const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/overseerLogModel.ts");
const helpers = import("../src/orchestrator/helpers.ts");

const entry = (over = {}) => ({
  ts: 1,
  repo: "/work/app",
  kind: "woke",
  text: "Woke for done",
  ...over,
});

test("addEntry keeps newest first, ignores a repeat and caps the list", async () => {
  const { addEntry } = await model;
  let list = addEntry([], entry({ ts: 1 }));
  list = addEntry(list, entry({ ts: 3, kind: "pr" }));
  list = addEntry(list, entry({ ts: 2, kind: "answered" }));
  assert.deepEqual(
    list.map((e) => e.ts),
    [3, 2, 1],
  );
  assert.equal(addEntry(list, entry({ ts: 3, kind: "pr" })), list);
  assert.equal(addEntry(list, entry({ ts: 9 }), 2).length, 2);
});

test("the filter lists each project once by name and narrows the rows", async () => {
  const { projectOptions, filterEntries } = await model;
  const list = [
    entry({ repo: "/work/zeta" }),
    entry({ repo: "/work/app" }),
    entry({ repo: "/work/app", ts: 2 }),
  ];
  assert.deepEqual(
    projectOptions(list).map((o) => o.name),
    ["app", "zeta"],
  );
  assert.equal(filterEntries(list, "/work/app").length, 2);
  assert.equal(filterEntries(list, null).length, 3);
});

test("the spend line hides the budget when it is 0", async () => {
  const { spentLine } = await model;
  assert.equal(spentLine(1.2, 5), "Spent today $1.20 of $5.00");
  assert.equal(spentLine(1.2, 0), "Spent today $1.20");
});

test("a digest or owner note opens the chat, other rows their task", async () => {
  const { rowTarget } = await model;
  assert.equal(rowTarget(entry({ kind: "digest" })), "chat");
  assert.equal(rowTarget(entry({ kind: "asked_owner", taskId: "t" })), "chat");
  assert.equal(rowTarget(entry({ kind: "pr", taskId: "t" })), "task");
  assert.equal(rowTarget(entry({ kind: "skipped" })), null);
});

test("an overseer event does not touch the live task state", async () => {
  const { applyOrchestratorEvent } = await helpers;
  const state = { tasks: [], logLines: {} };
  assert.equal(
    applyOrchestratorEvent(state, { event: "overseer", entry: entry() }),
    state,
  );
});

test("a same-millisecond entry with other text is kept, a repeat is not", async () => {
  const { addEntry } = await model;
  const a = entry({ ts: 5, taskId: "t", kind: "answered", text: "a" });
  const list = addEntry([], a);
  assert.equal(addEntry(list, { ...a }), list);
  assert.equal(addEntry(list, { ...a, text: "b" }).length, 2);
});

test("a pull request reads as its number and every text is capped", async () => {
  const { displayText } = await model;
  const pr = entry({
    kind: "pr",
    text: "https://github.example/acme/app/pull/7",
  });
  assert.equal(displayText(pr), "Opened PR #7");
  assert.equal(displayText(entry({ text: "x".repeat(500) })).length, 200);
});

test("rows group by local day with Today and Yesterday labels", async () => {
  const { groupByDay } = await model;
  const now = new Date(2026, 9, 10, 15).getTime();
  const at = (d, h) => new Date(2026, 9, d, h).getTime();
  const groups = groupByDay(
    [
      entry({ ts: at(10, 9) }),
      entry({ ts: at(10, 8) }),
      entry({ ts: at(9, 8) }),
      entry({ ts: at(7, 8) }),
    ],
    now,
  );
  assert.deepEqual(
    groups.map((g) => [g.label, g.entries.length]),
    [
      ["Today", 2],
      ["Yesterday", 1],
      ["Oct 7", 1],
    ],
  );
});
