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

test("formatDuration reads in whole minutes and never prints a zero unit", async () => {
  const { formatDuration } = await library;
  assert.equal(formatDuration(0), "<1m");
  assert.equal(formatDuration(-500), "<1m");
  assert.equal(formatDuration(45_000), "<1m");
  assert.equal(formatDuration(125_000), "2m");
  assert.equal(formatDuration(20 * 60_000), "20m");
  assert.equal(formatDuration(2 * 3_600_000), "2h");
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
    formatTaskTier(task({ tier: "standard", tierFallback: "no planner tier" })),
    "standard tier (fallback: no planner tier)",
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

test("a waiting row shows the real question count", async () => {
  const { taskReason } = await library;
  const waiting = (text) =>
    taskReason(
      task({ status: "waiting", question: { text, options: [] } }),
      [],
    );
  assert.equal(
    waiting("The planner has 3 blocking question(s); answer them all:\n\n1. a"),
    "3 questions for you",
  );
  assert.equal(waiting("Which one?"), "1 question for you");
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

test("settingsDifferingFromDefaults never flags routes", async () => {
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
  });
  const defaults = settings();

  const planner = resetSettingToDefault(saved, defaults, "planner");
  assert.equal(planner.planner, "claude-opus");
  assert.deepEqual(settingsDifferingFromDefaults(planner, defaults), [
    "tiers.standard",
  ]);

  const tier = resetSettingToDefault(saved, defaults, "tiers.standard");
  assert.deepEqual(tier.tiers, {
    mechanical: "codex",
    standard: "claude-sonnet",
    hard: "claude-opus",
  });

  // The input itself is never mutated.
  assert.equal(saved.planner, "claude-sonnet");
});

function variant(overrides = {}) {
  return {
    stallTimeoutSecs: 120,
    reviewEvidence: false,
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
  const taskVariant = variant({ stallTimeoutSecs: 300, reviewEvidence: true });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), experiments),
    "stallTimeoutSecs 300 · reviewEvidence on",
  );
});

test("variantLabel leaves an unchanged flag out even when it sits between two differing ones", async () => {
  const { variantLabel } = await library;
  const experiments = variant();
  const taskVariant = variant({
    stallTimeoutSecs: 300,
    reviewEvidence: true,
  });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), experiments),
    "stallTimeoutSecs 300 · reviewEvidence on",
  );
});

test("variantLabel lists route overrides after the flags", async () => {
  const { variantLabel } = await library;
  const taskVariant = variant({
    reviewEvidence: true,
    plannerRoute: "claude-sonnet",
    tierRoutes: { hard: "claude-sonnet" },
  });
  assert.equal(
    variantLabel(task({ variant: taskVariant }), variant()),
    "reviewEvidence on · plannerRoute claude-sonnet · tierRoutes hard=claude-sonnet",
  );
  assert.equal(
    variantLabel(task({ variant: variant() }), taskVariant),
    "reviewEvidence off · plannerRoute settings · tierRoutes settings",
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

test("owner Inbox rows search by title or project and open the task at its question or summary", async () => {
  const { matchesOwnerTask, ownerTarget } =
    await import("../src/orchestrator/ownerAttention.ts");
  const task = {
    id: "t1",
    title: "Ship it",
    repo: "/work/Alpha",
    status: "waiting",
  };
  assert.ok(matchesOwnerTask(task, "ship"));
  assert.ok(matchesOwnerTask(task, "alpha"));
  assert.ok(!matchesOwnerTask(task, "beta"));
  assert.deepEqual(ownerTarget(task), {
    taskId: "t1",
    repo: "/work/Alpha",
    focus: "question",
  });
  assert.equal(ownerTarget({ ...task, status: "failed" }).focus, "summary");
});

const NOON = new Date(2026, 8, 29, 12, 0, 0).getTime();
const YESTERDAY = NOON - 24 * 3600 * 1000;

test("railGroups sorts top-level tasks into needs you, running, landed today and earlier", async () => {
  const { railGroups } = await library;
  const tasks = [
    task({ id: "w", status: "waiting", updatedAt: NOON - 1 }),
    task({ id: "f", status: "failed", updatedAt: NOON - 2 }),
    task({ id: "r", status: "running", updatedAt: NOON - 3 }),
    task({ id: "q", status: "queued", updatedAt: NOON - 4 }),
    task({ id: "today", status: "done", landedSha: "a1", updatedAt: NOON }),
    task({ id: "old", status: "done", landedSha: "a2", updatedAt: YESTERDAY }),
    task({ id: "unlanded", status: "done", updatedAt: NOON }),
    task({ id: "archived", status: "waiting", archived: true }),
    task({ id: "child", status: "running", parent: "r" }),
    task({ id: "orphan", status: "running", parent: "gone", updatedAt: 1 }),
  ];
  const groups = railGroups(tasks, NOON);
  const ids = (list) => list.map((t) => t.id);
  assert.deepEqual(ids(groups.needsYou), ["w", "f"]);
  assert.deepEqual(ids(groups.running), ["r", "q", "orphan"]);
  assert.deepEqual(ids(groups.landedToday), ["today"]);
  assert.deepEqual(ids(groups.earlier), ["unlanded", "old"]);
});

test("isLandedToday needs a landed commit and an update since midnight", async () => {
  const { isLandedToday } = await library;
  const midnight = new Date(2026, 8, 29).getTime();
  const done = { status: "done", landedSha: "abc" };
  assert.equal(
    isLandedToday(task({ ...done, updatedAt: midnight }), NOON),
    true,
  );
  assert.equal(
    isLandedToday(task({ ...done, updatedAt: midnight - 1 }), NOON),
    false,
  );
  assert.equal(
    isLandedToday(task({ status: "done", updatedAt: NOON }), NOON),
    false,
  );
});

test("childrenOf nests a parent's subtasks and subtaskState names each one's state", async () => {
  const { childrenOf, subtaskState, taskReason } = await library;
  const parent = task({ id: "p", status: "running" });
  const lease = task({
    id: "a",
    parent: "p",
    status: "done",
    landedSha: "x",
    createdAt: 1,
  });
  const relay = task({
    id: "b",
    parent: "p",
    status: "running",
    createdAt: 2,
    attempts: [attempt()],
  });
  const ui = task({
    id: "c",
    parent: "p",
    status: "queued",
    dependsOn: ["b"],
    createdAt: 3,
  });
  const tasks = [parent, ui, relay, lease];
  assert.deepEqual(
    childrenOf(tasks, "p").map((t) => t.id),
    ["a", "b", "c"],
  );
  assert.deepEqual(subtaskState(lease, tasks, 4), {
    tone: "ok",
    label: "landed",
  });
  assert.deepEqual(subtaskState(relay, tasks, 4), {
    tone: "info",
    label: "implement · 1/4",
  });
  assert.deepEqual(subtaskState(ui, tasks, 4), {
    tone: "neutral",
    label: "waits",
  });
  assert.equal(taskReason(parent, tasks, 4), "3 subtasks · 1 landed");
});

test("taskReason and taskMetaLine give a rail row its second and third lines", async () => {
  const { taskReason, taskMetaLine } = await library;
  const waiting = task({
    status: "waiting",
    question: { text: "Which theme?", options: [], kind: "agent_question" },
    attempts: [attempt({ status: "blocked" })],
    costUsd: 0.04,
  });
  assert.equal(taskReason(waiting, [waiting], 4), "1 question for you");
  assert.equal(taskMetaLine(waiting, 4, NOON), "attempt 1/4 · $0.04");
  const failed = task({
    status: "failed",
    costUsd: 0.61,
    attempts: [1, 2, 3, 4].map((n) =>
      attempt({
        n,
        status: "failed",
        failure: { kind: "verify", detail: "npm test", signature: "s" },
      }),
    ),
  });
  assert.equal(taskReason(failed, [failed], 4), "verify failed · 4/4 attempts");
  assert.equal(taskMetaLine(failed, 4, NOON), "$0.61");
  const running = task({
    status: "running",
    costUsd: 0.22,
    attempts: [
      attempt({
        n: 1,
        status: "failed",
        startedAt: NOON - 600_000,
        endedAt: NOON - 360_000,
      }),
      attempt({ n: 2, startedAt: NOON - 120_000 }),
    ],
  });
  assert.equal(taskReason(running, [running], 4), "implement · attempt 2/4");
  assert.equal(taskMetaLine(running, 4, NOON), "6m · $0.22");
  const queued = task({
    status: "queued",
    queueReason: "queued behind default-landing",
  });
  assert.equal(
    taskReason(queued, [queued], 4),
    "queued behind default-landing",
  );
  const landed = task({
    status: "done",
    landedSha: "a",
    baseRef: "feature/x",
    costUsd: 0.31,
  });
  assert.equal(taskReason(landed, [landed], 4), "landed on feature/x");
});

test("stageTrack marks every stage before the current one done and the rest pending", async () => {
  const { stageTrack } = await library;
  const states = (t) =>
    stageTrack(t)
      .map((s) => `${s.stage}:${s.state}`)
      .join(" ");
  assert.equal(
    states(task({ status: "drafting" })),
    "brief:active implement:pending verify:pending review:pending land:pending",
  );
  assert.equal(
    states(task({ status: "running", attempts: [attempt()] })),
    "brief:done implement:active verify:pending review:pending land:pending",
  );
  assert.equal(
    states(
      task({
        status: "running",
        attempts: [
          attempt({
            verify: [{ command: "npm test", code: null, tail: "", ms: 0 }],
          }),
        ],
      }),
    ),
    "brief:done implement:done verify:active review:pending land:pending",
  );
  assert.equal(
    states(
      task({
        status: "waiting",
        question: { text: "?", options: [], kind: "attempts_failing" },
        attempts: [attempt({ status: "blocked" })],
      }),
    ),
    "brief:done implement:done verify:blocked review:pending land:pending",
  );
  assert.equal(
    states(
      task({
        status: "running",
        attempts: [attempt(), attempt({ n: 2, stage: "review" })],
      }),
    ),
    "brief:done implement:done verify:done review:active land:pending",
  );
  assert.equal(
    states(
      task({
        status: "failed",
        attempts: [
          attempt({
            status: "failed",
            failure: { kind: "review", detail: "", signature: "" },
          }),
        ],
      }),
    ),
    "brief:done implement:done verify:done review:failed land:pending",
  );
  assert.equal(
    states(task({ status: "done", attempts: [attempt({ status: "passed" })] })),
    "brief:done implement:done verify:done review:done land:pending",
  );
  assert.equal(
    states(task({ status: "done", landedSha: "abc" })),
    "brief:done implement:done verify:done review:done land:done",
  );
});

test("planDrafts lists unstarted tasks, never lease-queued or already implementing ones", async () => {
  const { planDrafts } = await library;
  const tasks = [
    task({ id: "draft", status: "queued" }),
    task({
      id: "parked",
      status: "stopped",
      attempts: [attempt({ stage: "plan", status: "passed" })],
    }),
    task({ id: "lease", status: "queued", queueReason: "waits for a lease" }),
    task({
      id: "retry",
      status: "queued",
      attempts: [attempt({ status: "failed" })],
    }),
    task({ id: "gone", status: "queued", archived: true }),
  ];
  assert.deepEqual(
    planDrafts(tasks).map((t) => t.id),
    ["draft", "parked"],
  );
});

test("unreadChatCount counts only assistant replies after the last look", async () => {
  const { unreadChatCount } = await library;
  const messages = [
    { role: "user", ts: 5 },
    { role: "assistant", ts: 6 },
    { role: "assistant", ts: 9 },
  ];
  assert.equal(unreadChatCount(messages, 6), 1);
  assert.equal(unreadChatCount(messages, 0), 2);
});

test("errorText strips Electron's IPC wrapper", async () => {
  const { errorText } = await library;
  assert.equal(
    errorText(
      new Error(
        "Error invoking remote method 'orchestrator': Error: connect ECONNREFUSED",
      ),
    ),
    "connect ECONNREFUSED",
  );
});

test("landRepos and withLandRepos edit the per-repo landing list", async () => {
  const { landRepos, withLandRepos } = await library;
  assert.deepEqual(landRepos(undefined), []);
  assert.deepEqual(landRepos({ "/a": true, "/b": false }), ["/a"]);
  assert.deepEqual(withLandRepos({ "/a": true, "/b": false }, ["/c"]), {
    "/b": false,
    "/c": true,
  });
  assert.deepEqual(withLandRepos(undefined, []), {});
});

test("daemonDown tells a gone daemon from a refused request", async () => {
  const { daemonDown } = await library;
  assert.equal(
    daemonDown(
      "The orchestrator daemon is not built. Run npm run build:orchd.",
    ),
    "not-built",
  );
  assert.equal(
    daemonDown("connect ECONNREFUSED /tmp/orchd.sock"),
    "unavailable",
  );
  assert.equal(
    daemonDown("The orchestrator daemon disconnected before responding."),
    "unavailable",
  );
  assert.equal(daemonDown("task is running"), null);
});

test("the composer chip and the header name the route the same way", async () => {
  const { orchestratorRouteLabel } = await library;
  assert.equal(orchestratorRouteLabel(settings()), "Claude Sonnet");
  assert.equal(
    orchestratorRouteLabel(settings({ orchestrator: "codex" })),
    "Codex",
  );
  assert.equal(
    orchestratorRouteLabel(settings({ orchestrator: "gone" })),
    "gone",
  );
});
