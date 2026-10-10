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

test("the log model has no project filter: the view asks for one repo", async () => {
  const m = await model;
  assert.equal(m.projectOptions, undefined);
  assert.equal(m.filterEntries, undefined);
});

test("the spend query is this project's UTC day and only its entries are kept", async () => {
  const { spentTodayQuery, belongsToProject } = await model;
  const late = Date.UTC(2026, 9, 8, 23, 30);
  assert.deepEqual(spentTodayQuery("/work/app", late), {
    repo: "/work/app",
    from: "2026-10-08",
    to: "2026-10-08",
    groupBy: [],
  });
  assert.deepEqual(spentTodayQuery(undefined, late), {
    from: "2026-10-08",
    to: "2026-10-08",
    groupBy: [],
  });
  assert.equal(belongsToProject(entry(), "/work/app"), true);
  assert.equal(
    belongsToProject(entry({ repo: "/work/other" }), "/work/app"),
    false,
  );
});

test("the spend line shows the project and the global total against the budget, or the project alone", async () => {
  const { spentLine } = await model;
  assert.equal(
    spentLine(1.2, 3, 5),
    "This project $1.20 \u00b7 all $3.00 of $5.00",
  );
  assert.equal(spentLine(1.2, 3, 0), "Spent today $1.20");
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

test("an opened row shows the full detail, else its own text", async () => {
  const { detailText } = await model;
  assert.equal(
    detailText(
      entry({ text: "Plan question → yes", detail: "Q?\n\nAnswer: yes\n" }),
    ),
    "Q?\n\nAnswer: yes",
  );
  assert.equal(detailText(entry({ text: " old row " })), "old row");
});

test("one row is open at a time and a second click closes it", async () => {
  const { toggleOpen, entryKey } = await model;
  assert.equal(toggleOpen(null, "a"), "a");
  assert.equal(toggleOpen("a", "b"), "b");
  assert.equal(toggleOpen("a", "a"), null);
  assert.notEqual(entryKey(entry({ ts: 1 })), entryKey(entry({ ts: 2 })));
  assert.notEqual(entryKey(entry({ taskId: "t1" })), entryKey(entry()));
});

test("the full date and time carries the year and the clock", async () => {
  const { fullDateTime } = await model;
  const text = fullDateTime(new Date(2026, 9, 8, 15, 5).getTime());
  assert.match(text, /Oct 8, 2026/);
  assert.match(text, /3:05\s?PM/);
});
