const { test } = require("node:test");
const assert = require("node:assert/strict");

const kinds = import("../src/orchestrator/chatKinds.ts");
const model = import("../src/orchestrator/chatModel.ts");

const REPO = "/repo";

/** A client whose chat calls answer like orchd's chat.rs: one current
 * session per kind, summaries carrying their kind. */
function fakeClient() {
  const calls = [];
  const sessions = {
    chat: [
      { id: "c1", kind: "chat", busy: false, updatedAt: 1, messageCount: 2 },
    ],
    brainstorm: [
      {
        id: "b1",
        kind: "brainstorm",
        busy: false,
        updatedAt: 1,
        messageCount: 0,
      },
    ],
  };
  const thread = (kind) => ({
    repo: REPO,
    id: sessions[kind][0].id,
    kind,
    createdAt: 1,
    messages: [],
    busy: false,
  });
  return {
    calls,
    chatGet: async (repo, kind) => {
      calls.push(["chat.get", repo, kind]);
      return thread(kind);
    },
    chatList: async (repo, kind) => {
      calls.push(["chat.list", repo, kind]);
      return { current: sessions[kind][0].id, sessions: sessions[kind] };
    },
  };
}

test("entering a kind asks orchd for that kind's current session and list", async () => {
  const k = await kinds;
  const client = fakeClient();
  const { thread, list } = await k.enterKind(REPO, "brainstorm", client);
  assert.equal(thread.id, "b1");
  assert.equal(thread.kind, "brainstorm");
  assert.deepEqual(
    list.sessions.map((s) => s.id),
    ["b1"],
  );
  assert.deepEqual(client.calls.map((c) => c[2]).sort(), [
    "brainstorm",
    "brainstorm",
  ]);
});

test("isOfKind reads the kind orchd put on the summary", async () => {
  const k = await kinds;
  const list = {
    current: "b1",
    sessions: [
      {
        id: "b1",
        kind: "brainstorm",
        busy: false,
        updatedAt: 1,
        messageCount: 0,
      },
    ],
  };
  assert.equal(k.isOfKind(list, "b1", "brainstorm"), true);
  assert.equal(k.isOfKind(list, "b1", "chat"), false);
  assert.equal(k.isOfKind(list, "unknown", "chat"), true);
  assert.equal(k.isOfKind(null, "unknown", "brainstorm"), false);
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

test("an older daemon's chat event without a kind is a chat event", async () => {
  const { eventKind } = await kinds;
  assert.equal(eventKind({ event: "chat" }), "chat");
  assert.equal(eventKind({ event: "chat", kind: "brainstorm" }), "brainstorm");
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
