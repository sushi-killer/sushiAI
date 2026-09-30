const { test } = require("node:test");
const assert = require("node:assert/strict");

const stats = import("../src/orchestrator/stats.ts");
const NOW = new Date(2026, 8, 29, 15, 0, 0).getTime();
const DAY = 24 * 60 * 60 * 1000;

function task(overrides = {}) {
  return {
    id: "t",
    title: "T",
    repo: "/r",
    status: "done",
    decisions: [],
    attempts: [],
    costUsd: 0,
    createdAt: NOW - DAY,
    updatedAt: NOW - DAY,
    ...overrides,
  };
}

test("weekSummary counts the cohort and formats the one-liner", async () => {
  const { weekSummary } = await stats;
  const tasks = [
    task({ id: "a", landedSha: "x", reportAt: NOW - DAY + 30 * 60000 }),
    task({ id: "b", landedSha: "x" }),
    task({ id: "c" }),
    task({
      id: "d",
      status: "failed",
      questionHistory: [
        { answeredBy: "policy", askedAt: 0, answeredAt: 1, answer: "a" },
      ],
    }),
    task({ id: "old", createdAt: NOW - 20 * DAY, landedSha: "x" }),
    task({ id: "sub", parent: "a" }),
    task({ id: "draft", status: "drafting" }),
  ];
  const week = weekSummary(tasks, { totals: { costUsd: 12.4 } }, NOW);
  assert.equal(week.started, 4);
  assert.equal(week.landed, 2);
  assert.equal(week.waitingToLand, 1);
  assert.equal(week.questionsPerTask, 0.3);
  assert.equal(week.text, "2 of 4 landed · 0.3 questions per task · $12.40");
});

test("weekSummary drops the parts it cannot know", async () => {
  const { weekSummary } = await stats;
  assert.equal(weekSummary([], null, NOW).text, "0 of 0 landed");
});

test("periodStats: median to land runs from the first attempt to the report", async () => {
  const { periodStats, periodAt } = await stats;
  const start = NOW - DAY;
  const tasks = [
    task({ id: "a", landedSha: "x", reportAt: start + 10 * 60000 }),
    task({ id: "b", landedSha: "x", reportAt: start + 30 * 60000 }),
  ];
  const result = periodStats(tasks, periodAt(NOW));
  assert.equal(result.medianToLandMs, 20 * 60000);
  assert.equal(result.landedPercent, 100);
});

test("periodStats groups failures by the last failed attempt", async () => {
  const { periodStats, periodAt } = await stats;
  const failed = (id, kind) =>
    task({
      id,
      status: "failed",
      attempts: [{ startedAt: NOW - DAY, failure: { kind } }],
    });
  const result = periodStats(
    [failed("a", "verify"), failed("b", "verify"), failed("c", "review")],
    periodAt(NOW),
  );
  assert.deepEqual(result.failedBy, [
    { kind: "verify", count: 2 },
    { kind: "review", count: 1 },
  ]);
});

test("periodStats counts questions from questionHistory plus open ones", async () => {
  const { periodStats, periodAt } = await stats;
  const answered = (answeredBy, askedAt, answeredAt) => ({
    question: "?",
    options: [],
    kind: "agent_question",
    askedBy: "implement",
    askedAt,
    answer: "a",
    answeredAt,
    answeredBy,
  });
  const result = periodStats(
    [
      task({
        id: "a",
        questionHistory: [
          answered("owner", 0, 4 * 60000),
          answered("owner", 0, 2 * 60000),
          answered("policy", 0, 1),
          answered("orchestrator", 0, 1),
          // A question from before orchd stamped askedAt: no wait to measure.
          { ...answered("owner", 0, 9), askedAt: undefined },
        ],
        assumptions: [
          { by: "policy", overturned: true },
          { by: "planner", overturned: true },
        ],
      }),
      task({
        id: "b",
        status: "waiting",
        question: { text: "?", options: [], kind: "budget" },
      }),
    ],
    periodAt(NOW),
  );
  assert.equal(result.questions, 6);
  assert.equal(result.answeredForYou, 2);
  assert.equal(result.overturned, 1);
  assert.equal(result.medianWaitMs, 3 * 60000);
});

test("periodStats has no median wait when the owner answered nothing", async () => {
  const { periodStats, periodAt } = await stats;
  const result = periodStats([task()], periodAt(NOW));
  assert.equal(result.questions, 0);
  assert.equal(result.medianWaitMs, null);
});

test("spendRange windows costs.summary by the period's UTC day keys", async () => {
  const { spendRange, dayKeys, periodAt } = await stats;
  const period = periodAt(NOW, 1);
  const keys = dayKeys(period);
  assert.deepEqual(spendRange(period), { from: keys[0], to: keys[6] });
  assert.match(spendRange(period).from, /^\d{4}-\d{2}-\d{2}$/);
});

test("the land insight only claims a dirty checkout for landing tasks", async () => {
  const { landInsight, landingTasks } = await stats;
  const landing = landingTasks([
    task({ id: "l", status: "landing" }),
    task({ id: "sub", status: "landing", parent: "l" }),
    task({ id: "d" }),
  ]);
  assert.deepEqual(
    landing.map((t) => t.id),
    ["l"],
  );
  assert.equal(landInsight(2, []), "2 finished tasks are ready to land");
  assert.equal(landInsight(1, []), "1 finished task is ready to land");
  assert.equal(
    landInsight(0, landing),
    "1 finished task waits for a clean checkout",
  );
  assert.equal(
    landInsight(3, landing),
    "3 finished tasks are ready to land · 1 finished task waits for a clean checkout",
  );
  assert.equal(landInsight(0, []), "");
});

test("spendBars fills missing days and names today", async () => {
  const { spendBars, periodAt, dayKeys } = await stats;
  const period = periodAt(NOW);
  const keys = dayKeys(period);
  assert.equal(keys.length, 7);
  const bars = spendBars(
    { rows: [{ key: keys[6], costUsd: 1.4 }] },
    period,
    true,
  );
  assert.equal(bars.length, 7);
  assert.equal(bars[6].label, "Today");
  assert.equal(bars[6].costUsd, 1.4);
  assert.equal(bars[0].costUsd, 0);
});

test("periodAt moves back whole periods", async () => {
  const { periodAt } = await stats;
  assert.equal(periodAt(NOW, 1).to, periodAt(NOW).from);
});

test("signed prints direction and hides zero", async () => {
  const { signed } = await stats;
  assert.equal(signed(4, String), "+4");
  assert.equal(
    signed(-0.12, (n) => `$${n.toFixed(2)}`),
    "−$0.12",
  );
  assert.equal(signed(0, String), null);
});
