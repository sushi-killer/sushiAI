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

test("the recurring count is the real total, not the shown top ten", async () => {
  const { recurringFailures, recurringTotal, failuresHint, sectionLabel } =
    await model;
  const rows = Array.from({ length: 21 }, (_, i) =>
    failure({ signature: `s${i}`, count: 2 + (i % 3) }),
  );
  rows.push(failure({ signature: "once", count: 1 }));
  assert.equal(recurringFailures(rows).length, 10);
  assert.equal(recurringTotal(rows), 21);
  assert.equal(
    sectionLabel("RECURRING FAILURES", 21, failuresHint(21)),
    "RECURRING FAILURES · 21 — last 14 days, showing 10",
  );
  assert.equal(failuresHint(4), "last 14 days");
});

test("the run summary says how many proposer runs started or that nothing qualified", async () => {
  const { runSummary } = await model;
  assert.equal(
    runSummary({ started: [{ id: "a" }, { id: "b" }], updated: [] }),
    "Started 2 proposer runs; proposals appear here as they finish.",
  );
  assert.match(
    runSummary({ started: [{ id: "a" }] }),
    /^Started 1 proposer run;/,
  );
  for (const empty of [{ started: [] }, {}, null, undefined, "x"]) {
    assert.equal(
      runSummary(empty),
      "Nothing has enough evidence for a proposal yet.",
    );
  }
});

test("the rail badge adds recurring failures to proposals waiting on the owner", async () => {
  const { improvementsBadge } = await model;
  const proposals = [
    { id: "a", status: "proposed" },
    { id: "b", status: "revert_suggested" },
    { id: "c", status: "adopted" },
    { id: "d", status: "approved" },
  ];
  assert.equal(improvementsBadge(proposals, 5), 7);
  assert.equal(improvementsBadge([], 0), 0);
});

const precedent = (over = {}) => ({
  id: "p1",
  kind: "preexisting_failure",
  title: "Base check failing",
  signals: [],
  commands: ["cargo test"],
  answer: "retry after the base is fixed",
  by: "owner",
  why: "",
  count: 1,
  firstAt: 1,
  lastAt: 1,
  ...over,
});

test("precedent rows name the kind, the answerer, the why and a repeat count", async () => {
  const { precedentRow, questionKindLabel, countSuffix } = await model;
  assert.equal(questionKindLabel("preexisting_failure"), "Base check failing");
  assert.equal(questionKindLabel("agent_question"), "Agent question");
  assert.equal(questionKindLabel("something_new"), "Something new");
  assert.equal(countSuffix(1), "");
  assert.equal(countSuffix(2), "×2");
  assert.deepEqual(
    precedentRow(precedent({ count: 2, why: "the base moves daily" })),
    {
      kind: "Base check failing (cargo test)",
      answer: "retry after the base is fixed",
      by: "answered by owner",
      why: "the base moves daily",
      count: "×2",
    },
  );
  const kinds = [["cargo test"], ["npm test", "cargo clippy"], []].map(
    (commands) => precedentRow(precedent({ commands })).kind,
  );
  assert.deepEqual(kinds, [
    "Base check failing (cargo test)",
    "Base check failing (npm test, cargo clippy)",
    "Base check failing",
  ]);
  for (const by of ["lead", "overseer", "policy"]) {
    assert.equal(precedentRow(precedent({ by })).by, `answered by ${by}`);
  }
});

test("recent precedents are newest first and capped at 20", async () => {
  const { recentPrecedents, MAX_PRECEDENTS } = await model;
  const rows = Array.from({ length: 25 }, (_, i) =>
    precedent({ id: `p${i}`, lastAt: i }),
  );
  const recent = recentPrecedents(rows);
  assert.equal(recent.length, MAX_PRECEDENTS);
  assert.equal(recent[0].id, "p24");
  assert.equal(recent.at(-1).id, "p5");
  assert.equal(rows[0].id, "p0");
});

test("the decision memory section label counts its precedents", async () => {
  const { sectionLabel } = await model;
  assert.equal(
    sectionLabel("DECISION MEMORY", 3, "how you answered similar questions"),
    "DECISION MEMORY · 3 — how you answered similar questions",
  );
});
