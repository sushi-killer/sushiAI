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

function message(overrides = {}) {
  return {
    id: "m1",
    repo: "/Users/sushi/project",
    from: "orchestrator",
    to: "t1",
    kind: "message",
    text: "Go ahead",
    ts: 1000,
    delivered: false,
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

test("taskCreateParams adds a trimmed base on the request path, omits the key when blank", async () => {
  const { taskCreateParams } = await library;
  assert.deepEqual(
    taskCreateParams({ request: "Add CSV export", start: true }, "  main  "),
    { request: "Add CSV export", start: true, base: "main" },
  );
  assert.deepEqual(
    taskCreateParams({ request: "Add CSV export", start: true }, ""),
    { request: "Add CSV export", start: true },
  );
  assert.deepEqual(
    taskCreateParams({ request: "Add CSV export", start: true }, "   "),
    { request: "Add CSV export", start: true },
  );
});

test("taskCreateParams adds a trimmed base on the title/goal fallback path too", async () => {
  const { taskCreateParams } = await library;
  assert.deepEqual(
    taskCreateParams(
      { title: "Add CSV export", goal: "Add CSV export", start: true },
      "feature/base-demo",
    ),
    {
      title: "Add CSV export",
      goal: "Add CSV export",
      start: true,
      base: "feature/base-demo",
    },
  );
  assert.deepEqual(
    taskCreateParams(
      { title: "Add CSV export", goal: "Add CSV export", start: true },
      "",
    ),
    { title: "Add CSV export", goal: "Add CSV export", start: true },
  );
});

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

test("formatTaskTier shows just the tier when it wasn't a fallback", async () => {
  const { formatTaskTier } = await library;
  assert.equal(formatTaskTier(task({ tier: "standard" })), "standard tier");
});

test("formatTaskTier names the fallback reason next to the tier", async () => {
  const { formatTaskTier } = await library;
  assert.equal(
    formatTaskTier(
      task({ tier: "standard", tierFallback: "no classifier key" }),
    ),
    "standard tier (fallback: no classifier key)",
  );
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

test("latestImplementAttempt skips a plan attempt, even when it's the newest one", async () => {
  const { latestImplementAttempt } = await library;
  const plan = attempt({ n: 1, stage: "plan", status: "passed" });
  const implement = attempt({ n: 2, stage: "implement" });
  assert.equal(
    latestImplementAttempt(task({ attempts: [plan, implement] })),
    implement,
  );
  // A plan-only task (still drafting) has no implement attempt yet.
  assert.equal(latestImplementAttempt(task({ attempts: [plan] })), undefined);
});

test("implementAttemptCount counts only implementation attempts", async () => {
  const { implementAttemptCount } = await library;
  const plan = attempt({ n: 1, stage: "plan", status: "passed" });
  const firstImplement = attempt({ n: 2, stage: "implement" });
  const secondImplement = attempt({ n: 3, stage: "implement" });

  assert.equal(implementAttemptCount(task()), 0);
  assert.equal(implementAttemptCount(task({ attempts: [plan] })), 0);
  assert.equal(
    implementAttemptCount(task({ attempts: [plan, firstImplement] })),
    1,
  );
  assert.equal(
    implementAttemptCount(
      task({ attempts: [plan, firstImplement, secondImplement] }),
    ),
    2,
  );
});

test("statusDetail adds only what the pill and progress don't say, never the question text", async () => {
  const { statusDetail } = await library;
  assert.equal(statusDetail(task({ status: "drafting" })), "");
  assert.equal(statusDetail(task({ status: "queued" })), "");
  assert.equal(
    statusDetail(
      task({ status: "running", costUsd: 0.41, attempts: [attempt({ n: 2 })] }),
    ),
    "$0.41",
  );
  assert.equal(
    statusDetail(
      task({
        status: "waiting",
        question: {
          text: "Delete old keys now or in 2 releases?",
          options: [],
        },
      }),
    ),
    "1 question for you",
  );
  assert.equal(statusDetail(task({ status: "done", costUsd: 0.18 })), "$0.18");
  assert.equal(
    statusDetail(
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
    "review PASS · $0.18",
  );
  assert.equal(
    statusDetail(
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
    "npm test failed",
  );
  assert.equal(statusDetail(task({ status: "stopped" })), "");
});

test("attemptProgress counts the implement attempt while running or queued, null otherwise", async () => {
  const { attemptProgress } = await library;
  assert.equal(
    attemptProgress(
      task({ status: "running", attempts: [attempt({ n: 2 })] }),
      4,
    ),
    "1/4",
  );
  // The daemon's attempt numbers are global across stages, so the first
  // implementation is n=2 after a plan at n=1.
  assert.equal(
    attemptProgress(
      task({
        status: "running",
        attempts: [
          attempt({ n: 1, stage: "plan", status: "passed" }),
          attempt({ n: 2, stage: "implement" }),
        ],
      }),
      4,
    ),
    "1/4",
  );
  // No maxAttempts passed - falls back to the attempt's own number.
  assert.equal(
    attemptProgress(task({ status: "running", attempts: [attempt({ n: 1 })] })),
    "1/1",
  );
  // Queued but nothing has run yet: no implementation attempt has started.
  assert.equal(attemptProgress(task({ status: "queued" }), 5), "0/5");
  assert.equal(attemptProgress(task({ status: "done" }), 5), null);
  assert.equal(attemptProgress(task({ status: "waiting" }), 5), null);
});

test("statusBadgeLabel names each status a short word for the list row's pill", async () => {
  const { statusBadgeLabel } = await library;
  assert.equal(statusBadgeLabel(task({ status: "running" })), "Running");
  assert.equal(statusBadgeLabel(task({ status: "waiting" })), "Waiting");
  assert.equal(statusBadgeLabel(task({ status: "done" })), "Done");
  assert.equal(statusBadgeLabel(task({ status: "failed" })), "Failed");
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
  // A passed plan attempt is not an implement pass - drafting a plan (or a
  // "Draft only" task sitting on one) must not tick every criterion.
  assert.equal(
    criteriaMet(
      task({
        status: "running",
        attempts: [
          attempt({ n: 1, stage: "plan", status: "passed" }),
          attempt({ n: 1, stage: "implement", status: "failed" }),
        ],
      }),
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

test("applyOrchestratorEvent leaves the task state alone for a chat event", async () => {
  const { applyOrchestratorEvent, emptyLiveState } = await library;
  const next = applyOrchestratorEvent(emptyLiveState, {
    event: "chat",
    thread: { repo: "/r", messages: [], busy: true },
  });
  assert.equal(next, emptyLiveState);
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

test("applyOrchestratorEvent leaves the task state alone for a message event", async () => {
  const { applyOrchestratorEvent, emptyLiveState } = await library;
  const next = applyOrchestratorEvent(emptyLiveState, {
    event: "message",
    message: message(),
  });
  assert.equal(next, emptyLiveState);
});

test("upsertMessage replaces an existing message by id (a delivery flip) and keeps ts ascending", async () => {
  const { upsertMessage } = await library;
  const a = message({ id: "a", ts: 1000, delivered: false });
  const b = message({ id: "b", ts: 2000, delivered: false });
  let list = upsertMessage(
    [a, b],
    message({ id: "a", ts: 1000, delivered: true }),
  );
  assert.deepEqual(
    list.map((m) => [m.id, m.delivered]),
    [
      ["a", true],
      ["b", false],
    ],
  );
  // A brand-new id is appended and the list stays sorted by ts.
  list = upsertMessage(list, message({ id: "c", ts: 500, delivered: false }));
  assert.deepEqual(
    list.map((m) => m.id),
    ["c", "a", "b"],
  );
});

test("messageThreads groups by the unordered participant pair, newest thread first, messages oldest first", async () => {
  const { messageThreads } = await library;
  const threads = messageThreads([
    message({ id: "1", from: "t1", to: "orchestrator", ts: 1000 }),
    message({
      id: "2",
      from: "orchestrator",
      to: "t1",
      ts: 2000,
      kind: "reply",
      replyTo: "1",
      delivered: true,
    }),
    message({ id: "3", from: "t2", to: "orchestrator", ts: 1500 }),
  ]);
  assert.equal(threads.length, 2);
  // The t1<->orchestrator thread has the newest message (ts 2000), so it
  // sorts first even though t2's own message came earlier chronologically.
  assert.deepEqual(threads[0].participants.slice().sort(), [
    "orchestrator",
    "t1",
  ]);
  assert.deepEqual(
    threads[0].messages.map((m) => m.id),
    ["1", "2"],
  );
  assert.equal(threads[0].lastTs, 2000);
  assert.equal(threads[0].pending, 1); // only message "1" is undelivered
  assert.equal(threads[1].messages[0].id, "3");
  assert.equal(threads[1].pending, 1);
});

test("participantLabel names the orchestrator, a known task's title, or a short id fallback", async () => {
  const { participantLabel } = await library;
  const tasks = [task({ id: "abcdefgh12345", title: "Export CSV" })];
  assert.equal(participantLabel("orchestrator", tasks), "Orchestrator");
  assert.equal(participantLabel("abcdefgh12345", tasks), "Export CSV");
  assert.equal(participantLabel("zzzzzzzzunknown", tasks), "Task zzzzzzzz");
});

function settings(overrides = {}) {
  return {
    routes: [
      { id: "claude-sonnet", label: "Claude Sonnet", harness: "claude" },
      { id: "claude-opus", label: "Claude Opus", harness: "claude" },
      { id: "codex", label: "Codex", harness: "codex" },
    ],
    tiers: {
      mechanical: "codex",
      standard: "claude-sonnet",
      hard: "claude-opus",
    },
    review: "auto",
    classifier: {
      backend: "openrouter",
      model: "typesafe/jev-1.13",
      providerId: "",
    },
    sandbox: "native",
    allowedDomains: ["*"],
    codexNetwork: false,
    protectedPaths: [],
    maxAttempts: 4,
    parallel: 2,
    planner: "claude-opus",
    orchestrator: "",
    autoAnswer: false,
    experiments: {},
    prices: {},
    ...overrides,
  };
}

test("settingsDifferingFromDefaults flags a planner saved before the default changed", async () => {
  const { settingsDifferingFromDefaults } = await library;
  assert.deepEqual(
    settingsDifferingFromDefaults(
      settings({ planner: "claude-sonnet" }),
      settings(),
    ),
    ["planner"],
  );
});

test("settingsDifferingFromDefaults flags nothing for identical settings", async () => {
  const { settingsDifferingFromDefaults } = await library;
  assert.deepEqual(settingsDifferingFromDefaults(settings(), settings()), []);
  // Equal lists compare by value, not by reference.
  assert.deepEqual(
    settingsDifferingFromDefaults(
      settings({ allowedDomains: ["*"] }),
      settings({ allowedDomains: ["*"] }),
    ),
    [],
  );
});

test("settingsDifferingFromDefaults never flags routes or the classifier's provider", async () => {
  const { settingsDifferingFromDefaults } = await library;
  assert.deepEqual(
    settingsDifferingFromDefaults(
      settings({
        routes: [{ id: "mine", label: "Mine", harness: "codex" }],
      }),
      settings(),
    ),
    [],
  );
  assert.deepEqual(
    settingsDifferingFromDefaults(
      settings({
        classifier: {
          backend: "openrouter",
          model: "typesafe/jev-1.13",
          providerId: "openrouter-key",
        },
      }),
      settings(),
    ),
    [],
  );
  // Nor the fields the panel doesn't show.
  assert.deepEqual(
    settingsDifferingFromDefaults(
      settings({ experiments: { retry: "fresh" }, prices: { x: {} } }),
      settings(),
    ),
    [],
  );
});

test("settingsDifferingFromDefaults flags each compared field on its own", async () => {
  const { settingsDifferingFromDefaults } = await library;
  const differs = (overrides) =>
    settingsDifferingFromDefaults(settings(overrides), settings());
  assert.deepEqual(
    differs({
      tiers: {
        mechanical: "claude-sonnet",
        standard: "claude-opus",
        hard: "codex",
      },
    }),
    ["tiers.mechanical", "tiers.standard", "tiers.hard"],
  );
  assert.deepEqual(
    differs({
      classifier: { backend: "none", model: "other", providerId: "" },
    }),
    ["classifier.backend", "classifier.model"],
  );
  assert.deepEqual(
    differs({
      review: "",
      orchestrator: "codex",
      autoAnswer: true,
      sandbox: "host",
      codexNetwork: true,
      allowedDomains: ["github.com"],
      protectedPaths: ["src/app/**"],
      maxAttempts: 6,
      parallel: 3,
    }),
    [
      "review",
      "orchestrator",
      "autoAnswer",
      "sandbox",
      "codexNetwork",
      "allowedDomains",
      "protectedPaths",
      "maxAttempts",
      "parallel",
    ],
  );
});

test("resetSettingToDefault restores one setting and leaves the rest alone", async () => {
  const { resetSettingToDefault, settingsDifferingFromDefaults } =
    await library;
  const saved = settings({
    planner: "claude-sonnet",
    tiers: {
      mechanical: "codex",
      standard: "claude-opus",
      hard: "claude-opus",
    },
    classifier: { backend: "none", model: "x", providerId: "key" },
  });
  const defaults = settings();

  const planner = resetSettingToDefault(saved, defaults, "planner");
  assert.equal(planner.planner, "claude-opus");
  assert.deepEqual(settingsDifferingFromDefaults(planner, defaults), [
    "tiers.standard",
    "classifier.backend",
    "classifier.model",
  ]);

  const tier = resetSettingToDefault(saved, defaults, "tiers.standard");
  assert.deepEqual(tier.tiers, {
    mechanical: "codex",
    standard: "claude-sonnet",
    hard: "claude-opus",
  });

  const backend = resetSettingToDefault(saved, defaults, "classifier.backend");
  assert.deepEqual(backend.classifier, {
    backend: "openrouter",
    model: "x",
    providerId: "key",
  });
  // The input itself is never mutated.
  assert.equal(saved.planner, "claude-sonnet");
});

function variant(overrides = {}) {
  return {
    retryMode: "resume",
    stallTimeoutSecs: 120,
    plannerTier: false,
    contract: false,
    reviewOtherFamily: false,
    reviewEvidence: false,
    deferHeavyChecks: false,
    leanOutput: false,
    ...overrides,
  };
}

test("variantLabel says 'not recorded' when the task predates variants", async () => {
  const { variantLabel } = await library;
  assert.equal(
    variantLabel(task({ variant: undefined }), variant()),
    "not recorded",
  );
});

test("variantLabel says 'default' when the task's variant matches the current experiments", async () => {
  const { variantLabel } = await library;
  assert.equal(
    variantLabel(task({ variant: variant() }), variant()),
    "default",
  );
});

test("variantLabel lists only the differing flags, in Variant's field order, booleans as on/off", async () => {
  const { variantLabel } = await library;
  const experiments = variant();
  const taskVariant = variant({ retryMode: "fresh", leanOutput: true });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), experiments),
    "retryMode fresh · leanOutput on",
  );
});

test("variantLabel leaves an unchanged flag out even when it sits between two differing ones", async () => {
  const { variantLabel } = await library;
  const experiments = variant();
  const taskVariant = variant({
    plannerTier: true,
    contract: false,
    reviewOtherFamily: true,
  });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), experiments),
    "plannerTier on · reviewOtherFamily on",
  );
});

test("variantLabel lists route overrides after the flags", async () => {
  const { variantLabel } = await library;
  const taskVariant = variant({
    leanOutput: true,
    plannerRoute: "claude-sonnet",
    tierRoutes: { hard: "claude-sonnet" },
  });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), variant()),
    "leanOutput on · plannerRoute claude-sonnet · tierRoutes hard=claude-sonnet",
  );
  assert.equal(
    variantLabel(task({ variant: variant() }), taskVariant),
    "leanOutput off · plannerRoute settings · tierRoutes settings",
  );
});

test("variantLabel lists a dollar budget last and treats absent as none", async () => {
  const { variantLabel } = await library;
  const budget = variant({ maxCostUsd: 2.5 });
  assert.equal(
    variantLabel(task({ variant: budget }), variant()),
    "maxCostUsd $2.50",
  );
  assert.equal(
    variantLabel(task({ variant: variant() }), budget),
    "maxCostUsd none",
  );
  assert.equal(
    variantLabel(task({ variant: variant({ maxCostUsd: 0 }) }), variant()),
    "default",
  );
});

test("costByStage splits plan, each implement attempt, and review into their own totals", async () => {
  const { costByStage } = await library;
  const t = task({
    costUsd: 0.3,
    attempts: [
      attempt({ n: 1, stage: "plan", costUsd: 0.02 }),
      attempt({ n: 2, stage: "implement", costUsd: 0.1, reviewCostUsd: 0.05 }),
      attempt({ n: 3, stage: "implement", costUsd: 0.08, reviewCostUsd: 0.05 }),
    ],
  });
  assert.deepEqual(costByStage(t), {
    plan: 0.02,
    implement: [
      { n: 2, costUsd: 0.1 },
      { n: 3, costUsd: 0.08 },
    ],
    review: 0.1,
    other: 0,
  });
});

test("costByStage puts a leftover auto-answer cost into other", async () => {
  const { costByStage } = await library;
  const t = task({
    costUsd: 0.2,
    attempts: [attempt({ n: 1, stage: "implement", costUsd: 0.1 })],
  });
  assert.deepEqual(costByStage(t), {
    plan: 0,
    implement: [{ n: 1, costUsd: 0.1 }],
    review: 0,
    other: 0.1,
  });
});

test("costByStage puts a review's cost into other when the attempt has no reviewCostUsd", async () => {
  const { costByStage } = await library;
  const t = task({
    costUsd: 0.25,
    attempts: [attempt({ n: 1, stage: "implement", costUsd: 0.1 })],
  });
  assert.deepEqual(costByStage(t), {
    plan: 0,
    implement: [{ n: 1, costUsd: 0.1 }],
    review: 0,
    other: 0.15,
  });
});

test("childrenOf lists a parent's subtasks oldest first and nothing else", async () => {
  const { childrenOf } = await library;
  const tasks = [
    task({ id: "b", parent: "p", createdAt: 2002 }),
    task({ id: "p", createdAt: 1000 }),
    task({ id: "a", parent: "p", createdAt: 2001 }),
    task({ id: "x", parent: "other", createdAt: 2000 }),
  ];
  assert.deepEqual(
    childrenOf(tasks, "p").map((t) => t.id),
    ["a", "b"],
  );
  assert.deepEqual(childrenOf(tasks, "a"), []);
});

test("dependencyTitles names what a task waits for and skips deleted ids", async () => {
  const { dependencyTitles } = await library;
  const tasks = [
    task({ id: "a", title: "Part A" }),
    task({ id: "b", title: "Part B", dependsOn: ["a", "gone"] }),
  ];
  assert.deepEqual(dependencyTitles(tasks[1], tasks), ["Part A"]);
  assert.deepEqual(dependencyTitles(tasks[0], tasks), []);
});
