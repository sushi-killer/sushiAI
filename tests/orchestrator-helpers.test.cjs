const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/orchestrator/helpers.ts");

function task(overrides = {}) {
  return {
    id: "t1",
    title: "Export CSV",
    goal: "Add CSV export",
    criteria: [],
    verify: [],
    repo: "/Users/sushi/project",
    worktree: "/Users/sushi/project.task-csv-export",
    branch: "task/csv-export",
    baseSha: "abc123",
    status: "running",
    tier: "standard",
    decisions: [],
    attempts: [],
    costUsd: 0,
    createdAt: 1000,
    updatedAt: 1000,
    ...overrides,
  };
}

function attempt(overrides = {}) {
  return {
    n: 1,
    stage: "implement",
    routeId: "r1",
    harness: "codex",
    model: "default",
    reason: "tier: standard",
    resumed: false,
    startedAt: 0,
    status: "running",
    changedFiles: [],
    verify: [],
    gateBlocks: 0,
    ...overrides,
  };
}

test("formatDuration renders the coarsest unit that fits, zero for anything non-positive", async () => {
  const { formatDuration } = await library;
  assert.equal(formatDuration(0), "0s");
  assert.equal(formatDuration(-500), "0s");
  assert.equal(formatDuration(45_000), "45s");
  assert.equal(formatDuration(125_000), "2m 5s");
  assert.equal(formatDuration(3 * 3_600_000 + 61_000), "3h 1m");
});

test("formatCost always shows two decimals, including for an unset cost", async () => {
  const { formatCost } = await library;
  assert.equal(formatCost(undefined), "$0.00");
  assert.equal(formatCost(0.4), "$0.40");
  assert.equal(formatCost(12.3456), "$12.35");
});

test("attemptDurationMs uses `now` for a still-running attempt, endedAt once it has one", async () => {
  const { attemptDurationMs } = await library;
  const running = attempt({ startedAt: 1000 });
  assert.equal(attemptDurationMs(running, 4000), 3000);
  const done = attempt({ startedAt: 1000, endedAt: 2500 });
  assert.equal(attemptDurationMs(done, 999_999), 1500);
});

test("totalDurationMs sums every attempt of a task", async () => {
  const { totalDurationMs } = await library;
  const t = task({
    attempts: [
      attempt({ n: 1, startedAt: 0, endedAt: 1000 }),
      attempt({ n: 2, startedAt: 1000, endedAt: 4000 }),
    ],
  });
  assert.equal(totalDurationMs(t), 4000);
});

test("statusLabel names the attempt count and cost, never the question text", async () => {
  const { statusLabel } = await library;
  assert.equal(statusLabel(task({ status: "queued" })), "queued");
  assert.equal(
    statusLabel(
      task({
        status: "running",
        costUsd: 0.41,
        attempts: [attempt({ n: 2 })],
      }),
      4,
    ),
    "working · attempt 2/4 · $0.41",
  );
  // No maxAttempts passed - falls back to the attempt's own number.
  assert.equal(
    statusLabel(task({ status: "running", attempts: [attempt({ n: 1 })] })),
    "working · attempt 1/1 · $0.00",
  );
  assert.equal(
    statusLabel(
      task({
        status: "waiting",
        question: {
          text: "Delete old keys now or in 2 releases?",
          options: [],
        },
      }),
    ),
    "waiting for you · 1 question",
  );
  assert.equal(
    statusLabel(task({ status: "done", costUsd: 0.18 })),
    "done · $0.18",
  );
  assert.equal(
    statusLabel(
      task({
        status: "done",
        costUsd: 0.18,
        attempts: [
          attempt({
            n: 1,
            stage: "review",
            review: { verdict: "PASS", findings: [] },
          }),
        ],
      }),
    ),
    "done · review PASS · $0.18",
  );
  assert.equal(
    statusLabel(
      task({
        status: "stopped",
        attempts: [
          attempt({
            failure: {
              kind: "verify",
              detail: "npm test failed",
              signature: "x",
            },
          }),
        ],
      }),
    ),
    "stopped · npm test failed",
  );
  assert.equal(statusLabel(task({ status: "stopped" })), "stopped");
});

test("criteriaMet is true once the task is done, or once the latest attempt passed", async () => {
  const { criteriaMet } = await library;
  assert.equal(criteriaMet(task({ status: "running" })), false);
  assert.equal(criteriaMet(task({ status: "done" })), true);
  assert.equal(
    criteriaMet(
      task({ status: "running", attempts: [attempt({ status: "passed" })] }),
    ),
    true,
  );
  assert.equal(
    criteriaMet(
      task({ status: "running", attempts: [attempt({ status: "failed" })] }),
    ),
    false,
  );
});

test("upsertTask replaces an existing task in place and keeps newest-updated first", async () => {
  const { upsertTask } = await library;
  const a = task({ id: "a", updatedAt: 1 });
  const b = task({ id: "b", updatedAt: 2 });
  let list = upsertTask(
    [a, b],
    task({ id: "a", updatedAt: 5, title: "Renamed" }),
  );
  assert.deepEqual(
    list.map((t) => [t.id, t.updatedAt, t.title]),
    [
      ["a", 5, "Renamed"],
      ["b", 2, "Export CSV"],
    ],
  );
  // A brand-new id is appended, not silently dropped.
  list = upsertTask(list, task({ id: "c", updatedAt: 3 }));
  assert.deepEqual(
    list.map((t) => t.id),
    ["a", "c", "b"],
  );
});

test("applyOrchestratorEvent folds a task patch and caps accumulated log lines", async () => {
  const { applyOrchestratorEvent, emptyLiveState } = await library;
  let state = applyOrchestratorEvent(emptyLiveState, {
    event: "task",
    task: task({ id: "a" }),
  });
  assert.equal(state.tasks.length, 1);
  // The original state object is left untouched - callers pass the result on.
  assert.equal(emptyLiveState.tasks.length, 0);

  for (let i = 0; i < 205; i++)
    state = applyOrchestratorEvent(state, {
      event: "log",
      taskId: "a",
      attempt: 1,
      line: `line ${i}`,
    });
  assert.equal(state.logLines.a.length, 200);
  assert.equal(state.logLines.a[0], "line 5");
  assert.equal(state.logLines.a[199], "line 204");
});
