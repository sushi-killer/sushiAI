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
  effortChoices,
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
  parseAsk,
  readableTurn,
  initialAnswers,
  askReplyText,
  isAnswered,
} = require("../src/routines/model.ts");
const { composerHeight } = require("../src/ui/composerSize.ts");
const {
  attentionFromEvent,
  attentionFromEntries,
} = require("../src/routines/attention.ts");

const routine = (over = {}) => ({
  id: "digest",
  name: "Digest",
  where: { host: "my-mac", projects: ["my-app"] },
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
  efforts: {
    claude: ["low", "medium", "high", "xhigh", "max"],
    codex: ["low", "medium", "high"],
  },
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
  for (const harness of ["claude", "codex"])
    assert.deepEqual(effortsFor(harness, CATALOG), CATALOG.efforts[harness]);
  // No catalog offers nothing; the picker then keeps the current level.
  assert.deepEqual(effortsFor("claude", null), []);
  assert.deepEqual(effortChoices("claude", null, "high"), ["high"]);
  assert.deepEqual(
    effortChoices("codex", CATALOG, "high"),
    CATALOG.efforts.codex,
  );
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
  assert.deepEqual(payload.routine.where.projects, ["my-app"]);
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

// ---- the builder's questions ----

const askBlock = (json) => "```ask\n" + JSON.stringify(json) + "\n```";
const when = {
  id: "when",
  type: "choice",
  label: "Run time",
  options: [
    { value: "09:00", label: "Weekdays 09:00" },
    { value: "18:00", label: "Weekdays 18:00" },
  ],
};

test("parseAsk reads a valid block and keeps the prose", () => {
  const text = `Which time?\n\n${askBlock({ title: "Schedule", questions: [when] })}`;
  const { body, ask } = parseAsk(text);
  assert.equal(body, "Which time?");
  assert.equal(ask.title, "Schedule");
  assert.equal(ask.questions[0].options.length, 2);
  assert.equal(ask.questions[0].allowOther, false);
});

test("parseAsk keeps the whole text when the card is unusable", () => {
  const bad = [
    "```ask\n{not json}\n```",
    askBlock({ questions: [] }),
    askBlock({ title: "empty" }),
    askBlock({
      actions: [
        { id: "a", label: "A" },
        { id: "a", label: "B" },
      ],
    }),
    askBlock({ actions: [{ id: "a", label: 5 }] }),
    askBlock({
      actions: Array.from({ length: 5 }, (_, n) => ({
        id: `a${n}`,
        label: `A${n}`,
      })),
    }),
    askBlock({ questions: [when, when] }),
    askBlock({ questions: [{ ...when, type: "radio" }] }),
    askBlock({ questions: [{ ...when, label: 7 }] }),
    askBlock({ questions: [{ ...when, options: [] }] }),
    askBlock({ questions: [{ ...when, options: "09:00" }] }),
    askBlock({ questions: [{ ...when, allowOther: "yes" }] }),
    askBlock({
      questions: [
        {
          ...when,
          options: Array.from({ length: 9 }, (_, n) => ({
            value: `v${n}`,
            label: `Option ${n}`,
          })),
        },
      ],
    }),
    askBlock({ questions: [{ ...when, label: "x".repeat(500) }] }),
    askBlock({
      questions: Array.from({ length: 5 }, (_, n) => ({
        id: `q${n}`,
        type: "text",
        label: `Q${n}`,
      })),
    }),
    "```ask\n[1,2]\n```",
  ];
  for (const block of bad) {
    const text = `Question\n${block}`;
    assert.deepEqual(parseAsk(text), { body: text, ask: null }, block);
  }
  assert.deepEqual(parseAsk("plain A) B) C)"), {
    body: "plain A) B) C)",
    ask: null,
  });
});

test("parseAsk finds a block in the middle and uses the last of two", () => {
  const first = askBlock({
    questions: [{ id: "a", type: "text", label: "First" }],
  });
  const second = askBlock({
    questions: [{ id: "b", type: "text", label: "Second" }],
  });
  const middle = parseAsk(`Before\n${first}\nAfter`);
  assert.equal(middle.ask.questions[0].id, "a");
  assert.equal(middle.body, "Before\n\nAfter");
  const two = parseAsk(`${first}\ntext\n${second}`);
  assert.equal(two.ask.questions[0].id, "b");
  assert.ok(two.body.includes("First"));
});

test("askReplyText writes one plain line per answered question", () => {
  const { ask } = parseAsk(
    askBlock({
      questions: [
        { ...when, default: "09:00", allowOther: true },
        {
          id: "days",
          type: "multi",
          label: "Days",
          options: [
            { value: "mon", label: "Mon" },
            { value: "tue", label: "Tue" },
          ],
        },
        { id: "note", type: "text", label: "Note" },
      ],
    }),
  );
  const answers = initialAnswers(ask);
  assert.equal(askReplyText(ask, answers), "Run time: Weekdays 09:00");
  assert.equal(isAnswered(ask.questions[1], answers.days), false);
  answers.days = { picked: ["mon", "tue"], other: "" };
  answers.note = { picked: [], other: " keep it short " };
  answers.when = { picked: [], other: "10:30" };
  assert.equal(
    askReplyText(ask, answers),
    "Run time: 10:30\nDays: Mon, Tue\nNote: keep it short",
  );
});

test("parseAsk reads actions; an unknown kind is only a reply button", () => {
  const { ask } = parseAsk(
    askBlock({
      actions: [
        {
          id: "ok",
          label: "Looks good",
          kind: "reply",
          reply: "Save it",
          style: "primary",
        },
        { id: "dry", label: "Dry run", kind: "dry-run" },
        { id: "open", label: "Open", kind: "open-routine" },
        { id: "bad", label: "Wipe it", kind: "rm -rf", reply: "x" },
      ],
    }),
  );
  assert.equal(ask.questions.length, 0);
  assert.deepEqual(
    ask.actions.map((a) => [a.kind, a.reply, a.style]),
    [
      ["reply", "Save it", "primary"],
      ["dry-run", "Dry run", "secondary"],
      ["open-routine", "Open", "secondary"],
      ["reply", "Wipe it", "secondary"],
    ],
  );
  const both = parseAsk(
    askBlock({
      questions: [{ id: "n", type: "text", label: "N" }],
      actions: [{ id: "a", label: "A" }],
    }),
  );
  assert.equal(both.ask.questions.length, 1);
  assert.equal(both.ask.actions.length, 1);
});

test("readableTurn turns a run-result JSON reply into markdown", () => {
  const payload = {
    status: "needs_you",
    title: "Routine \u201cMorning plan\u201d saved as a draft, access needed",
    summary: "It reads the channel at 09:00. Grant the access to run it.",
  };
  const json = JSON.stringify(payload);
  const out = readableTurn(json);
  assert.equal(out, `**${payload.title}**\n\n${payload.summary}`);
  assert.equal(readableTurn("```json\n" + json + "\n```"), out);
  assert.equal(readableTurn(`Done.\n\n${json}`), `Done.\n\n${out}`);
  assert.match(
    readableTurn(JSON.stringify({ ...payload, next: "Turn it on" })),
    /\n\nTurn it on$/,
  );
});

test("readableTurn leaves everything else untouched and never throws", () => {
  for (const text of [
    "",
    "plain prose",
    '{"status":"needs_you","title":"x"',
    'Use {"a": 1} here, then more text',
    '{"name":"x","value":2}',
    '{"status":"ok"}',
    '{"title":"x","summary":"y"}',
    "[1,2]",
    "{",
  ])
    assert.equal(readableTurn(text), text);
});

test("where: summary text, toggling and host change", () => {
  const {
    describeWhere,
    toggleProject,
    keepOnHost,
    whereProblem,
    projectsForHost,
    MAX_PROJECTS,
  } = require("../src/routines/where.ts");
  const catalog = {
    hosts: [],
    projects: [
      {
        id: "p1",
        name: "my-app",
        folders: [{ host: "my-mac", path: "/a" }],
        remote: "",
      },
      {
        id: "p2",
        name: "my-site",
        folders: [{ host: "build-host", path: "/b" }],
        remote: "",
      },
      {
        id: "p3",
        name: "my-docs",
        folders: [],
        remote: "https://example.com/d.git",
      },
    ],
  };
  assert.equal(describeWhere({}, catalog, "My Mac"), "My Mac");
  assert.equal(describeWhere({ projects: [] }, catalog, "My Mac"), "My Mac");
  assert.equal(
    describeWhere({ projects: ["p1"] }, catalog, "My Mac"),
    "My Mac · my-app",
  );
  assert.equal(
    describeWhere({ projects: ["my-app"] }, catalog, "My Mac"),
    "My Mac · my-app",
  );
  assert.equal(
    describeWhere({ projects: ["p1", "p2", "p3"] }, catalog, "My Mac"),
    "My Mac · 3 projects",
  );

  assert.deepEqual(toggleProject([], catalog, "p1"), ["p1"]);
  assert.deepEqual(toggleProject(["p1"], catalog, "p2"), ["p1", "p2"]);
  assert.deepEqual(toggleProject(["p1", "p2"], catalog, "p1"), ["p2"]);
  assert.deepEqual(toggleProject(["my-app"], catalog, "p1"), []);
  const full = Array.from({ length: MAX_PROJECTS }, (_, i) => `x${i}`);
  assert.equal(toggleProject(full, catalog, "p1").length, MAX_PROJECTS);

  // p2 has no folder on my-mac and no remote: it does not follow the move.
  assert.deepEqual(
    projectsForHost(catalog, "my-mac").map((p) => p.id),
    ["p1", "p3"],
  );
  assert.deepEqual(keepOnHost(["p1", "p2", "p3"], catalog, "my-mac"), [
    "p1",
    "p3",
  ]);

  assert.match(whereProblem("codex", ["a", "b"]), /needs Claude/);
  assert.equal(whereProblem("codex", ["a"]), "");
  assert.equal(whereProblem("claude", ["a", "b"]), "");
});
