const { test } = require("node:test");
const assert = require("node:assert/strict");

const client = import("../src/orchestrator/client.ts");
const model = import("../src/orchestrator/chatModel.ts");

const REPO = "/repo";

const proposal = {
  tasks: [
    { title: "Add the table", goal: "g", criteria: ["a", "b"], dependsOn: [] },
    {
      title: "Use the table",
      goal: "g",
      criteria: ["c"],
      dependsOn: [1, "t9"],
      tier: "hard",
    },
    { title: "Document it", goal: "g", criteria: [], dependsOn: [2] },
  ],
};

test("the chat client sends the mode with a message and creates proposals by row", async () => {
  const { orchestratorClientFor } = await client;
  const calls = [];
  globalThis.window = {
    bridge: {
      orchestrator: async (method, params) => {
        calls.push([method, params]);
        return {};
      },
    },
  };
  try {
    const c = orchestratorClientFor("local");
    await c.chatSend(REPO, "hi");
    await c.chatSend(REPO, "an idea", "brainstorm");
    await c.chatCreateProposal(REPO, "m1", {
      indices: [1, 2],
      skip: [3],
      backlog: true,
    });
    await c.chatGet(REPO);
    assert.deepEqual(calls, [
      ["chat.send", { repo: REPO, text: "hi", mode: "chat" }],
      ["chat.send", { repo: REPO, text: "an idea", mode: "brainstorm" }],
      [
        "chat.createProposal",
        {
          repo: REPO,
          messageId: "m1",
          indices: [1, 2],
          skip: [3],
          backlog: true,
        },
      ],
      ["chat.get", { repo: REPO }],
    ]);
  } finally {
    delete globalThis.window;
  }
});

test("a reply's mode label is its mode's name, none for the plain chat", async () => {
  const { modeLabel, CHAT_MODES } = await model;
  assert.deepEqual(
    CHAT_MODES.map((m) => m.label),
    ["Chat", "Brainstorm", "Plan"],
  );
  assert.equal(modeLabel(undefined), "");
  assert.equal(modeLabel("chat"), "");
  assert.equal(modeLabel("brainstorm"), "Brainstorm");
  assert.equal(modeLabel("plan"), "Plan");
});

test("the placeholder follows the mode, and a brainstorm that asked is answered", async () => {
  const { composerPlaceholder } = await model;
  assert.equal(composerPlaceholder("chat", []), "Ask the orchestrator…");
  assert.equal(composerPlaceholder("plan", []), "Describe the goal to plan…");
  assert.equal(composerPlaceholder("brainstorm", []), "Describe an idea…");
  const asked = [
    { role: "user", mode: "brainstorm" },
    {
      role: "assistant",
      mode: "brainstorm",
      questions: [{ text: "Who?", options: ["a", "b"] }],
    },
  ];
  assert.equal(
    composerPlaceholder("brainstorm", asked),
    "Answer, or push back on the list…",
  );
  // A chat-mode reply with questions does not count as a brainstorm asking.
  assert.equal(
    composerPlaceholder("brainstorm", [
      { role: "assistant", questions: [{ text: "?", options: [] }] },
    ]),
    "Describe an idea…",
  );
});

test("a proposal row's meta reads tier, criteria and what it waits for", async () => {
  const { proposalMeta } = await model;
  const tasks = [{ id: "t9", title: "Existing task" }];
  assert.equal(proposalMeta(proposal.tasks[0], proposal, tasks), "2 criteria");
  assert.equal(
    proposalMeta(proposal.tasks[1], proposal, tasks),
    "hard · 1 criterion · after Add the table, Existing task",
  );
  assert.equal(
    proposalMeta(proposal.tasks[2], proposal, tasks),
    "0 criteria · after Use the table",
  );
  // An unknown existing task is left out rather than shown as an id.
  assert.equal(
    proposalMeta(proposal.tasks[1], proposal, []),
    "hard · 1 criterion · after Add the table",
  );
});

test("row states, the footer count and the create request", async () => {
  const { initialChecks, rowState, proposalSelection, proposalRequest } =
    await model;
  const p = {
    tasks: [
      { ...proposal.tasks[0], taskId: "made" },
      { ...proposal.tasks[1], skipped: true },
      proposal.tasks[2],
    ],
  };
  assert.deepEqual(initialChecks(p), [true, false, true]);
  const checked = initialChecks(p);
  assert.deepEqual(
    p.tasks.map((t, i) => rowState(t, checked[i])),
    ["created", "skipped", "open"],
  );
  // Unchecking an open row reads as skipped before anything is sent.
  assert.equal(rowState(p.tasks[2], false), "skipped");
  assert.deepEqual(proposalSelection(p, checked), { create: [3], skip: [2] });
  assert.deepEqual(proposalRequest(p, checked), {
    indices: [3],
    skip: [2],
    backlog: false,
  });
  assert.deepEqual(proposalRequest(p, checked, { backlog: true }), {
    indices: [3],
    skip: [2],
    backlog: true,
  });
  // One row's Create leaves the other checked rows open.
  const fresh = { tasks: proposal.tasks };
  assert.deepEqual(proposalRequest(fresh, [true, true, false], { row: 2 }), {
    indices: [2],
    skip: [3],
    backlog: false,
  });
  assert.equal(
    proposalRequest({ tasks: [{ ...proposal.tasks[0], taskId: "x" }] }, [true]),
    null,
  );
});

test("the footer button counts the checked rows that are not tasks yet", async () => {
  const { createLabel } = await model;
  assert.equal(createLabel(1), "Create 1 task");
  assert.equal(createLabel(3), "Create 3 tasks");
  assert.equal(createLabel(0), "Create 0 tasks");
});

test("only the orchestrator chat passes a mode to its composer", () => {
  const fs = require("node:fs");
  const dir = `${__dirname}/../src/orchestrator/`;
  const withMode = fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".tsx"))
    .filter((f) => /\bonModeChange=/.test(fs.readFileSync(dir + f, "utf8")));
  assert.deepEqual(withMode.sort(), ["ChatView.tsx"]);
  assert.ok(!fs.existsSync(dir + "BrainstormView.tsx"));
  assert.ok(!fs.existsSync(dir + "chatKinds.ts"));
});

test("sessions group newest first by updatedAt into TODAY and EARLIER", async () => {
  const { groupSessions } = await model;
  const now = new Date(2026, 8, 29, 12).getTime();
  const sessions = [
    { id: "old", updatedAt: now - 3 * 86_400_000 },
    { id: "a", updatedAt: now - 3_600_000 },
    { id: "yesterday", updatedAt: now - 20 * 3_600_000 },
    { id: "fresh", updatedAt: now - 60_000 },
  ];
  const groups = groupSessions(sessions, now);
  assert.deepEqual(
    groups.map((g) => [g.label, g.sessions.map((s) => s.id)]),
    [
      ["TODAY", ["fresh", "a"]],
      ["EARLIER", ["yesterday", "old"]],
    ],
  );
});

test("session times read like the design", async () => {
  const { sessionTime } = await model;
  const now = new Date(2026, 8, 29, 12).getTime();
  assert.equal(sessionTime(now - 20_000, now), "now");
  assert.equal(sessionTime(now - 2 * 60_000, now), "2m");
  assert.equal(sessionTime(now - 3 * 3_600_000, now), "3h");
  assert.equal(sessionTime(new Date(2026, 8, 28, 9).getTime(), now), "Mon");
  assert.equal(sessionTime(new Date(2026, 7, 3).getTime(), now), "Aug 3");
});

test("task references match known ids and long titles only", async () => {
  const { taskRefs } = await model;
  const tasks = [
    { id: "0f3a9c21-aaaa-bbbb", title: "Fix export of empty projects" },
    { id: "77777777-cccc", title: "Short" },
    { id: "12345678-dddd", title: "Remote hosts over SSH" },
  ];
  const text =
    "Task 12345678 is queued. Fix export of empty projects failed; Short answer: no.";
  assert.deepEqual(
    taskRefs(text, tasks).map((t) => t.title),
    ["Remote hosts over SSH", "Fix export of empty projects"],
  );
  assert.deepEqual(taskRefs("nothing here", tasks), []);
});

test("search matches any given field, case-insensitively", async () => {
  const { matchesQuery } = await model;
  assert.ok(matchesQuery("", "x"));
  assert.ok(matchesQuery("EXPORT", "Why did export fail?", undefined));
  assert.ok(!matchesQuery("spend", "Why did export fail?", "It failed"));
});

test("sessions without updatedAt sort last and never produce NaN", async () => {
  const { groupSessions, newestFirst, sessionTime } = await model;
  const now = new Date(2026, 8, 29, 12).getTime();
  const sessions = [
    { id: "untimed" },
    { id: "fresh", updatedAt: now - 60_000 },
    { id: "untimed2" },
    { id: "old", updatedAt: now - 3 * 86_400_000 },
  ];
  assert.deepEqual(
    [...sessions].sort(newestFirst).map((s) => s.id),
    ["fresh", "old", "untimed", "untimed2"],
  );
  for (const a of sessions)
    for (const b of sessions)
      assert.equal(Number.isNaN(newestFirst(a, b)), false);
  assert.deepEqual(
    groupSessions(sessions, now).map((g) => [
      g.label,
      g.sessions.map((s) => s.id),
    ]),
    [
      ["TODAY", ["fresh"]],
      ["EARLIER", ["old", "untimed", "untimed2"]],
    ],
  );
  assert.equal(sessionTime(undefined, now), "");
});

test("session previews and unread marks", async () => {
  const { lastMessageText, isUnread } = await model;
  assert.equal(
    lastMessageText({
      messages: [
        { text: "first" },
        { text: "It failed verify\n4 times" },
        { text: "  " },
      ],
    }),
    "It failed verify 4 times",
  );
  assert.equal(lastMessageText({ messages: [] }), "");
  // Never opened: only a change after the view loaded counts.
  assert.equal(isUnread({ updatedAt: 50 }, undefined, 100), false);
  assert.equal(isUnread({ updatedAt: 150 }, undefined, 100), true);
  assert.equal(isUnread({ updatedAt: 150 }, 200, 100), false);
  assert.equal(isUnread({}, undefined, 0), false);
});

test("several questions are answered as one message and read back from it", async () => {
  const { composeAnswers, pickedAnswers } = await model;
  const qs = [
    { text: "Which db?", options: ["pg", "sqlite"] },
    { text: "Which host?", options: ["local", "ssh"] },
    { text: "Docs?", options: ["yes", "no"] },
  ];
  assert.equal(composeAnswers(qs, []), null);
  const sent = composeAnswers(qs, ["sqlite", undefined, "no"]);
  assert.equal(sent, "Which db?: sqlite\nDocs?: no");
  assert.deepEqual(pickedAnswers(qs, sent), ["sqlite", undefined, "no"]);
  assert.deepEqual(pickedAnswers(qs, undefined), [
    undefined,
    undefined,
    undefined,
  ]);
  assert.deepEqual(pickedAnswers(qs, "Which db?: mysql"), [
    undefined,
    undefined,
    undefined,
  ]);
});

test("the empty chat hints read as the Figma concept", () => {
  const source = require("node:fs").readFileSync(
    `${__dirname}/../src/orchestrator/ChatView.tsx`,
    "utf8",
  );
  assert.ok(
    source.includes(
      '["Brainstorm", "turns an idea into tasks, one question at a time"]',
    ),
  );
  assert.ok(
    source.includes('["Plan", "splits a goal into ordered tasks you confirm"]'),
  );
});

test("editing an own message: Enter sends, Shift+Enter is a newline, Esc cancels, later messages dim", async () => {
  const { editKeyAction, canSendEdit, isAfterEdit } = await model;
  const key = (k, shiftKey = false, isComposing = false) =>
    editKeyAction({ key: k, shiftKey, isComposing });
  assert.equal(key("Enter"), "send");
  assert.equal(key("Enter", true), null);
  assert.equal(key("Escape"), "cancel");
  assert.equal(key("Enter", false, true), null);
  assert.equal(key("a"), null);
  assert.equal(canSendEdit("  ", false), false);
  assert.equal(canSendEdit("fix", true), false);
  assert.equal(canSendEdit("fix", false), true);
  const messages = [{ id: "a" }, { id: "b" }, { id: "c" }];
  assert.deepEqual(
    messages.map((_, i) => isAfterEdit(messages, "b", i)),
    [false, false, true],
  );
  assert.equal(isAfterEdit(messages, null, 2), false);
});

test("the chat client edits a message by id, sending the mode only when given", async () => {
  const { orchestratorClientFor } = await client;
  const calls = [];
  globalThis.window = {
    bridge: {
      orchestrator: async (method, params) => {
        calls.push([method, params]);
        return {};
      },
    },
  };
  try {
    const c = orchestratorClientFor("local");
    await c.chatEdit(REPO, "m1", "fixed");
    await c.chatEdit(REPO, "m1", "fixed", "plan");
    assert.deepEqual(calls, [
      ["chat.edit", { repo: REPO, messageId: "m1", text: "fixed" }],
      [
        "chat.edit",
        { repo: REPO, messageId: "m1", text: "fixed", mode: "plan" },
      ],
    ]);
  } finally {
    delete globalThis.window;
  }
});
