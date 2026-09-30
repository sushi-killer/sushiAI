const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/taskDetailModel.ts");

function attempt(overrides = {}) {
  return {
    n: 1,
    stage: "implement",
    routeId: "r",
    harness: "claude",
    model: "sonnet",
    reason: "",
    startedAt: 0,
    status: "passed",
    changedFiles: [],
    verify: [],
    gateBlocks: 0,
    ...overrides,
  };
}

function task(overrides = {}) {
  return {
    id: "t1",
    title: "T",
    goal: "",
    criteria: ["a", "b"],
    verify: [],
    repo: "/r",
    worktree: "/w",
    branch: "task/t",
    baseSha: "x",
    status: "done",
    tier: "standard",
    decisions: [],
    attempts: [],
    costUsd: 0.31,
    createdAt: 0,
    updatedAt: 1_000_000,
    ...overrides,
  };
}

function seg(stage, startedAt, endedAt, overrides = {}) {
  return {
    stage,
    attempt: 1,
    startedAt,
    endedAt,
    costUsd: 0,
    outcome: "",
    ...overrides,
  };
}

test("a failed verify segment is narrated from the attempt's results", async () => {
  const { segmentNarration } = await model;
  const t = task({
    attempts: [
      attempt({
        verify: [
          { command: "npm run ci", code: 1, tail: "a\n2 tests failed", ms: 1 },
        ],
      }),
      attempt({ n: 2 }),
    ],
  });
  const steps = segmentNarration(
    t,
    seg("verify", 0, 1000, { failureKind: "verify" }),
  );
  assert.deepEqual(
    steps.map((s) => s.kind),
    ["run", "fail", "next"],
  );
  assert.match(steps[1].text, /npm run ci exited 1 · 2 tests failed/);
  assert.match(steps[2].text, /sent back to implement/);
});

test("review and advisor segments read the verdict and the advice", async () => {
  const { segmentNarration } = await model;
  const t = task({
    attempts: [
      attempt({
        review: { verdict: "FAIL", findings: ["missing test"] },
        advice: "check the lease",
      }),
    ],
  });
  const review = segmentNarration(t, seg("review", 0, 1));
  assert.deepEqual(
    review.map((s) => s.text),
    ["reviewer said FAIL", "missing test"],
  );
  assert.equal(
    segmentNarration(t, seg("advisor", 0, 1))[0].text,
    "check the lease",
  );
});

test("a segment with nothing to narrate falls back to its outcome", async () => {
  const { segmentNarration } = await model;
  const steps = segmentNarration(task(), seg("plan", 0, 1, { outcome: "ok" }));
  assert.deepEqual(steps, [{ kind: "note", text: "ok" }]);
});

test("stage rows sum runs, time and cost in pipeline order", async () => {
  const { stageRows } = await model;
  const t = task({ attempts: [attempt({ changedFiles: ["a", "b", "c"] })] });
  const rows = stageRows(t, [
    seg("verify", 0, 60_000, { costUsd: 0.5, failureKind: "verify" }),
    seg("implement", 0, 120_000, {
      costUsd: 0.1,
      buckets: { explore: 4, process: 1, evidence: 0, verify: 0, task: 0 },
    }),
    seg("implement", 0, 60_000, { costUsd: 0.1 }),
  ]);
  assert.deepEqual(
    rows.map((r) => r.stage),
    ["implement", "verify"],
  );
  assert.equal(rows[0].detail, "2 runs · 4 files read · 3 edited");
  assert.equal(rows[0].ms, 180_000);
  assert.equal(rows[0].costUsd, 0.2);
  assert.equal(rows[1].detail, "1 run · 1 failed");
});

test("bar labels only wide segments; the failed one opens by default", async () => {
  const { barLabel, defaultSegment } = await model;
  const wide = seg("implement", 0, 540_000);
  const narrow = seg("verify", 0, 60_000);
  assert.equal(barLabel(wide, 1_000_000), "implement 9m");
  assert.equal(barLabel(narrow, 1_000_000), null);
  assert.equal(barLabel(seg("wait", 0, 900_000), 1_000_000), null);
  assert.equal(defaultSegment([wide, { ...narrow, failureKind: "verify" }]), 1);
  assert.equal(defaultSegment([wide]), null);
});

test("report line and question source only state what is known", async () => {
  const { reportLine, questionSource } = await model;
  const t = task({
    attempts: [
      attempt({ changedFiles: ["a", "b", "c"] }),
      attempt({
        n: 2,
        stage: "review",
        review: { verdict: "PASS", findings: [] },
      }),
    ],
  });
  assert.equal(reportLine(t), "3 files · review PASS");
  assert.equal(reportLine(task()), "");
  const asked = {
    text: "?",
    options: [],
    kind: "attempts_failing",
    askedBy: "verify",
    askedAt: 0,
  };
  assert.equal(questionSource(asked, 120_000), "asked by verify · 2m ago");
  assert.equal(questionSource(asked, 30_000), "asked by verify · just now");
  // A question from before orchd recorded who asked: nothing is guessed.
  assert.equal(
    questionSource({ text: "?", options: [], kind: "budget" }, 120_000),
    "",
  );
});

test("header facts are separate parts and a UUID branch is shortened", async () => {
  const { headerFacts, shortBranch } = await model;
  const done = task({
    attempts: [attempt({ startedAt: 0, endedAt: 20 * 60_000 })],
  });
  const facts = headerFacts(done, 4, 20 * 60_000);
  assert.equal(facts[0], "1 attempt");
  assert.ok(facts.every((part) => !part.startsWith("·")));
  assert.equal(facts.includes("$0.31"), true);
  assert.equal(
    shortBranch("task/b61481a2-89ef-4a5b-8c33-5261f696b5cf"),
    "task/b614…b5cf",
  );
  assert.equal(shortBranch("task/dark-mode"), "task/dark-mode");
});
