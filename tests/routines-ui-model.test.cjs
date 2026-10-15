// The Routines page's logic without React: list filters, status words, the
// agent pickers, the save payload and the Inbox rows. Synthetic data only.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  groupEntries,
  runModeOf,
  withRunMode,
  briefRows,
  routineState,
  modelsFor,
  withAgent,
  effortsFor,
  savePayload,
  describeWhen,
  actionCounts,
  safeLink,
  harnessProblem,
  rowSubtitle,
  runRowTitle,
  leadingHeading,
  durationText,
  fixCommand,
  missingNotice,
  ungrantedAccess,
  pendingActions,
  approvalWords,
} = require("../src/routines/model.ts");
const { composerHeight } = require("../src/ui/composerSize.ts");
const {
  attentionFromEvent,
  attentionFromEntries,
} = require("../src/routines/attention.ts");

const routine = (over = {}) => ({
  id: "digest",
  name: "Digest",
  where: { host: "my-mac", project: "my-app" },
  when: { cron: "0 9 * * 1-5", tz: "Europe/Berlin" },
  agent: {
    harness: "claude",
    model: "sonnet",
    effort: "medium",
    extraArgs: [],
  },
  howItRuns: { mode: "ask-first", promoteAfter: 5, accepted: 0 },
  limits: { timeoutMin: 30 },
  enabled: true,
  ...over,
});
const entry = (status, over = {}) => ({
  routine: routine(over.routine),
  lastRun: status
    ? { n: 3, kind: "scheduled", status, startedAt: 1, ...over.run }
    : null,
  running: false,
  consecutiveFailures: 0,
  host: "local",
  hostName: "This Mac",
  ...over.entry,
});

const CATALOG = {
  hosts: [
    {
      id: "local",
      name: "This Mac",
      state: "ready",
      routines: true,
      hostName: "my-mac",
      harnesses: {
        claude: {
          installed: true,
          loggedIn: true,
          models: [
            { id: "sonnet", label: "Sonnet" },
            { id: "opus", label: "Opus" },
          ],
        },
        codex: {
          installed: true,
          loggedIn: true,
          models: [{ id: "gpt-local", label: "GPT Local" }],
        },
      },
    },
    {
      id: "ssh:build-host",
      name: "build-host",
      state: "ready",
      routines: true,
      hostName: "bh",
      harnesses: {
        claude: { installed: true, loggedIn: false, models: [] },
        codex: {
          installed: true,
          loggedIn: true,
          models: [{ id: "gpt-remote", label: "GPT Remote" }],
        },
      },
    },
  ],
  projects: [],
  defaults: { agent: { harness: "claude", model: "sonnet", effort: "medium" } },
};

test("states and filters: needs you, ran, failed and paused", () => {
  const entries = [
    entry("needs_you", { routine: { id: "a" } }),
    entry("done", { routine: { id: "b" } }),
    entry("failed", { routine: { id: "c" } }),
    entry("failed", {
      routine: { id: "d", enabled: false },
      entry: { consecutiveFailures: 3 },
    }),
    entry("noop", { routine: { id: "e" } }),
    entry(null, { routine: { id: "f", enabled: false } }),
  ];
  assert.deepEqual(entries.map(routineState), [
    "needs_you",
    "ran",
    "failed",
    "paused",
    "ran",
    "draft",
  ]);
  const groups = groupEntries(entries).map((group) => [
    group.label,
    group.entries.map((item) => item.routine.id),
  ]);
  assert.deepEqual(groups, [
    ["Needs you", ["a", "c", "d"]],
    ["Active", ["b", "e"]],
    ["Off", ["f"]],
  ]);
});

test("a dry run never makes a routine need the owner; a live run shows as running", () => {
  const dry = entry("needs_you", { run: { kind: "dry" } });
  assert.equal(routineState(dry), "idle");
  assert.equal(
    routineState(entry("running", { entry: { running: true } })),
    "running",
  );
});

test("how it runs: the daemon's three modes, and an older daemon's two", () => {
  const base = entry("done").routine;
  const every = withRunMode(base, "every");
  assert.equal(every.howItRuns.mode, "ask-every-time");
  assert.equal(runModeOf(every), "every");
  const first = withRunMode(every, "first");
  assert.equal(first.howItRuns.mode, "ask-first");
  assert.equal(first.howItRuns.promoteAfter, 5);
  assert.equal(runModeOf(first), "first");
  const own = withRunMode(first, "own");
  assert.equal(own.howItRuns.mode, "on-its-own");
  assert.equal(runModeOf(own), "own");
  // The promote threshold survives a trip through "ask every time".
  assert.equal(withRunMode(every, "first").howItRuns.promoteAfter, 5);
  // An older daemon: "ask-first" that never promotes.
  assert.equal(
    runModeOf({
      ...base,
      howItRuns: { mode: "ask-first", promoteAfter: 0, accepted: 0 },
    }),
    "every",
  );
});

test("access: only the tools the owner has not granted wait", () => {
  const routine = {
    ...entry("done").routine,
    access: [
      { tool: "Read", label: "Read tasks", kind: "read", granted: true },
      { tool: "Write", label: "Create tasks", kind: "write", granted: false },
    ],
  };
  assert.deepEqual(
    ungrantedAccess(routine).map((item) => item.tool),
    ["Write"],
  );
  // A daemon that sends no list means nothing waits.
  assert.deepEqual(ungrantedAccess(entry("done").routine), []);
  // The list tagged onto the entry is used when the routine has none.
  assert.equal(
    ungrantedAccess(entry("done").routine, [
      { tool: "T", label: "T", kind: "read", granted: false },
    ]).length,
    1,
  );
});

test("waiting changes: proposals of the run, else the unapplied actions of a run that needs you", () => {
  const run = {
    n: 3,
    kind: "scheduled",
    status: "needs_you",
    proposals: [
      { status: "applied", proposal: { kind: "branch", branch: "a" } },
      { status: "pending", proposal: { kind: "pr", title: "Fix crash" } },
      { status: "pending", proposal: { kind: "pr", branch: "b-2" } },
    ],
  };
  assert.deepEqual(pendingActions(run), [
    { index: 1, kind: "pr", title: "Fix crash" },
    { index: 2, kind: "pr", title: "b-2" },
  ]);
  // A result action row is a report, never an approve button.
  const bare = { n: 4, kind: "scheduled", status: "needs_you" };
  assert.deepEqual(pendingActions(bare), []);
  // A dry run only says what it would do; nothing waits for an OK.
  assert.deepEqual(pendingActions({ ...run, kind: "dry" }), []);
});

test("approval words follow the kind and the number checked", () => {
  const tasks = [
    { index: 0, kind: "task", title: "a" },
    { index: 1, kind: "task", title: "b" },
    { index: 2, kind: "task", title: "c" },
  ];
  assert.deepEqual(approvalWords(tasks, 2), {
    heading: "wants to create 3 tasks",
    button: "Create 2 tasks",
  });
  assert.equal(approvalWords(tasks, 1).button, "Create 1 task");
  const mixed = [
    { index: 0, kind: "pr", title: "a" },
    { index: 1, kind: "branch", title: "b" },
  ];
  assert.equal(approvalWords(mixed, 2).button, "Make 2 changes");
});

test("the composer field grows to six lines, then scrolls, and shrinks back", () => {
  // One line of text: 16px line + 14px padding = 30px.
  assert.deepEqual(composerHeight(30, 16, 14), { height: 30, overflow: false });
  // Three lines.
  assert.deepEqual(composerHeight(62, 16, 14), { height: 62, overflow: false });
  // Exactly six lines is the cap and still does not scroll.
  assert.deepEqual(composerHeight(110, 16, 14), {
    height: 110,
    overflow: false,
  });
  // Ten lines: capped at six, and it scrolls.
  assert.deepEqual(composerHeight(174, 16, 14), {
    height: 110,
    overflow: true,
  });
  // An empty field never collapses below one line.
  assert.deepEqual(composerHeight(0, 16, 14), { height: 30, overflow: false });
});

test("brief rows: read is the first step, do the goal, result its section", () => {
  const rows = briefRows(
    "# Weekly\n\n## Goal\nWrite the weekly report.\n\n## Steps\n1. Read the **merged** PRs.\n2. Draft.\n\n## Result\nA message in #mobile.\n",
  );
  assert.deepEqual(rows, {
    read: "Read the merged PRs.",
    doing: "Write the weekly report.",
    result: "A message in #mobile.",
  });
});

test("list subtitle never repeats the name; run rows never repeat their pill", () => {
  const now = 10 * 3600e3;
  const same = entry("done", {
    routine: { name: "Digest" },
    run: { title: "digest", endedAt: now - 3 * 3600e3 },
  });
  assert.equal(rowSubtitle(same, now), "Done \u00b7 3 h ago");
  assert.equal(
    rowSubtitle(entry("done", { run: { title: "Seven PRs merged" } }), now),
    "Seven PRs merged",
  );
  assert.match(rowSubtitle(entry(null), now), /^Weekdays at 09:00/);
  const run = (over) => ({ n: 7, kind: "scheduled", status: "noop", ...over });
  assert.equal(runRowTitle(run({ title: "Nothing to do" })), "Run 7");
  assert.equal(
    runRowTitle(run({ status: "done", kind: "dry", title: "Dry run" })),
    "Run 7 \u00b7 dry run",
  );
  assert.equal(
    runRowTitle(run({ status: "done", title: "Weekly report" })),
    "Weekly report",
  );
});

test("run text helpers: heading, meta line, dry subtitle, fix command", () => {
  assert.equal(leadingHeading("\n# Weekly report\n\ntext"), "Weekly report");
  assert.equal(leadingHeading("text\n# Later"), "");
  assert.equal(
    durationText({
      n: 1,
      kind: "dry",
      status: "done",
      startedAt: 1000,
      endedAt: 73e3,
    }),
    "1 min 12 s",
  );
  assert.equal(
    fixCommand("Install it with `brew install gh` first."),
    "brew install gh",
  );
  assert.equal(fixCommand("No command here."), "");
  assert.equal(missingNotice(["gh"], "Run `brew install gh`"), "");
  assert.equal(
    missingNotice(["gh", "jq"], "Run `brew install gh`"),
    "Needs: jq",
  );
});

test("the agent picker offers only the models of that host's harness", () => {
  assert.deepEqual(
    modelsFor(CATALOG, "local", "claude").map((m) => m.id),
    ["sonnet", "opus"],
  );
  assert.deepEqual(
    modelsFor(CATALOG, "ssh:build-host", "codex").map((m) => m.id),
    ["gpt-remote"],
  );
  // Switching the harness drops a model the new harness does not list.
  const next = withAgent(routine(), { harness: "codex" }, CATALOG, "local");
  assert.equal(next.agent.harness, "codex");
  assert.equal(next.agent.model, "");
  // A model of the host's list stays.
  const kept = withAgent(
    routine({ agent: { harness: "codex", extraArgs: [] } }),
    { model: "gpt-local" },
    CATALOG,
    "local",
  );
  assert.equal(kept.agent.model, "gpt-local");
  // xhigh and max are Claude's alone.
  assert.deepEqual(effortsFor("codex"), ["low", "medium", "high"]);
  assert.ok(effortsFor("claude").includes("max"));
  const clipped = withAgent(
    routine({ agent: { harness: "claude", effort: "max", extraArgs: [] } }),
    { harness: "codex" },
    CATALOG,
    "local",
  );
  assert.equal(clipped.agent.effort, "medium");
  assert.match(
    harnessProblem(CATALOG, "ssh:build-host", "claude"),
    /Log in to Claude Code on build-host/,
  );
});

test("the save payload carries the harness, model, effort and extra args the daemon builds argv from", () => {
  const changed = withAgent(
    routine({
      agent: {
        harness: "claude",
        model: "sonnet",
        effort: "medium",
        extraArgs: ["--allowedTools", "Read"],
      },
    }),
    { model: "opus", effort: "high" },
    CATALOG,
    "local",
  );
  const payload = savePayload(changed, "Summarise issues.");
  assert.equal(payload.brief, "Summarise issues.");
  assert.deepEqual(payload.routine.agent, {
    harness: "claude",
    model: "opus",
    effort: "high",
    extraArgs: ["--allowedTools", "Read"],
  });
  assert.equal(payload.routine.where.project, "my-app");
});

test("Inbox rows: noop none, needs_you and failed one each, paused one", () => {
  const finished = (status, over = {}) => ({
    kind: "run_finished",
    routineId: "digest",
    name: "Digest",
    run: 4,
    status,
    inbox: status === "needs_you" || status === "failed",
    ...over,
  });
  assert.equal(attentionFromEvent(finished("noop"), "local"), null);
  assert.equal(attentionFromEvent(finished("done"), "local"), null);
  // The daemon's flag is the gate: a quiet run with the wrong status is not a row.
  assert.equal(
    attentionFromEvent(finished("noop", { inbox: true }), "local"),
    null,
  );
  const needs = attentionFromEvent(finished("needs_you"), "local");
  assert.equal(needs.kind, "answer");
  assert.match(needs.key, /routines:local:digest:run-4/);
  assert.equal(attentionFromEvent(finished("failed"), "local").kind, "decide");
  const paused = attentionFromEvent(
    {
      kind: "paused",
      routineId: "digest",
      name: "Digest",
      failures: 3,
      inbox: true,
    },
    "local",
  );
  assert.match(paused.key, /paused$/);
  assert.equal(
    attentionFromEvent(
      { kind: "run_started", routineId: "x", inbox: false },
      "local",
    ),
    null,
  );
});

test("a fresh list implies the same rows after a restart", () => {
  const rows = attentionFromEntries([
    entry("noop", { routine: { id: "a" } }),
    entry("done", { routine: { id: "b" } }),
    entry("needs_you", { routine: { id: "c" } }),
    entry("failed", { routine: { id: "d" } }),
    entry("failed", {
      routine: { id: "e", enabled: false },
      entry: { consecutiveFailures: 3 },
    }),
  ]);
  assert.deepEqual(
    rows.map((row) => row.key.split(":").slice(2).join(":")),
    ["c:run-3", "d:run-3", "e:paused"],
  );
});

test("result helpers: applied vs would-do counts, and only http links open", () => {
  const result = {
    status: "done",
    title: "t",
    summary: "s",
    actions: [
      { kind: "slack.post", text: "a", applied: true },
      { kind: "git.pr", text: "b", applied: false },
      { kind: "git.pr", text: "c", applied: false },
    ],
  };
  assert.deepEqual(actionCounts(result), { applied: 1, wouldDo: 2 });
  assert.equal(safeLink("https://example.com/x"), "https://example.com/x");
  assert.equal(safeLink("javascript:alert(1)"), null);
  assert.equal(
    describeWhen("0 9 * * 1-5", "Europe/Berlin"),
    "Weekdays at 09:00 (Europe/Berlin)",
  );
  assert.equal(describeWhen("*/15 * * * *"), "Every 15 min");
});
