const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/improvementsModel.ts");

const failure = (over = {}) => ({
  signature: "verify:snapshot mismatch",
  kind: "verify",
  count: 3,
  tasks: [
    { id: "a", title: "Fix export" },
    { id: "b", title: "Sidebar titles" },
  ],
  lastSeen: 1,
  exampleDetail: "snapshot mismatch in smoke.cjs\nsecond line",
  exampleTaskId: "b",
  ...over,
});

test("recurring failures keep repeats only, most frequent first", async () => {
  const { recurringFailures } = await model;
  const rows = [
    failure({ signature: "once", count: 1 }),
    failure({ signature: "twice", count: 2 }),
    failure({ signature: "thrice", count: 3 }),
  ];
  assert.deepEqual(
    recurringFailures(rows).map((r) => r.signature),
    ["thrice", "twice"],
  );
});

test("failure copy uses the real first line and the task titles", async () => {
  const { failureTitle, failureTasksLine, failureNoteDraft } = await model;
  assert.equal(
    failureTitle(failure()),
    "verify · snapshot mismatch in smoke.cjs",
  );
  assert.equal(
    failureTasksLine(failure()),
    "in 2 tasks: Fix export, Sidebar titles",
  );
  assert.equal(
    failureTasksLine(failure({ tasks: [{ id: "a", title: "One" }] })),
    "in 1 task: One",
  );
  assert.match(
    failureNoteDraft(failure()),
    /^Recurring verify failure \(3×\): snapshot mismatch in smoke.cjs/,
  );
  assert.equal(
    failureTitle(failure({ exampleDetail: "" })),
    "verify · verify:snapshot mismatch",
  );
});

test("seenIn reads before.tasks and hides when absent", async () => {
  const { seenIn } = await model;
  assert.equal(seenIn({ before: { tasks: 4, signals: 9 } }), "seen in 4 tasks");
  assert.equal(seenIn({ before: { tasks: 1, signals: 1 } }), "seen in 1 task");
  assert.equal(seenIn({}), "");
});

test("proposals upsert newest first and count only open ones", async () => {
  const { upsertProposal, openProposalCount } = await model;
  const a = { id: "a", status: "proposed" };
  const b = { id: "b", status: "adopted" };
  let rows = upsertProposal([], a);
  rows = upsertProposal(rows, b);
  assert.deepEqual(
    rows.map((r) => r.id),
    ["b", "a"],
  );
  rows = upsertProposal(rows, { ...a, status: "rejected" });
  assert.equal(rows.length, 2);
  assert.equal(openProposalCount(rows), 0);
  assert.equal(openProposalCount([a]), 1);
});

test("section labels carry the count and an optional hint", async () => {
  const { sectionLabel } = await model;
  assert.equal(sectionLabel("PROPOSALS", 2), "PROPOSALS · 2");
  assert.equal(
    sectionLabel("REPO NOTES", 3, "standing guidance for the planner"),
    "REPO NOTES · 3 — standing guidance for the planner",
  );
});
