const { test, beforeEach } = require("node:test");
const assert = require("node:assert/strict");

const kinds = import("../src/orchestrator/chatKinds.ts");
const model = import("../src/orchestrator/chatModel.ts");

const REPO = "/repo";

/** A daemon that keeps chat sessions the way orchd's chat.rs does today:
 * no kinds, one current session, switching refused while a turn is live. */
function fakeDaemon({ withKinds = false } = {}) {
  let n = 0;
  const fresh = () => ({ id: `s${++n}`, messages: [], busy: false });
  const state = { sessions: [fresh()], current: "s1", calls: [] };
  const current = () => state.sessions.find((s) => s.id === state.current);
  const thread = () => ({ ...current(), repo: REPO });
  const list = () => ({
    current: state.current,
    sessions: state.sessions.map((s) => ({
      id: s.id,
      busy: s.busy,
      ...(s.title ? { title: s.title } : {}),
      ...(withKinds ? { kind: s.kind ?? "chat" } : {}),
    })),
  });
  const idle = () => {
    if (current().busy) throw new Error("the orchestrator is still answering");
  };
  state.orchestrator = async (method, params) => {
    state.calls.push({ method, params });
    switch (method) {
      case "chat.list":
        return list();
      case "chat.get":
        return thread();
      case "chat.new": {
        idle();
        const s = fresh();
        if (withKinds) s.kind = params.kind;
        state.sessions.push(s);
        state.current = s.id;
        return thread();
      }
      case "chat.switch":
        idle();
        if (!state.sessions.some((s) => s.id === params.id))
          throw new Error("no such chat session");
        state.current = params.id;
        return thread();
      case "chat.send": {
        const s = current();
        if (!s.title) s.title = params.text.slice(0, 59);
        s.messages.push({
          id: `m${s.messages.length}`,
          role: "user",
          text: params.text,
          ts: 1,
        });
        return {};
      }
      default:
        throw new Error(`unexpected ${method}`);
    }
  };
  return state;
}

function install(daemon) {
  const store = new Map();
  globalThis.window = {
    localStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
    },
    bridge: { orchestrator: daemon.orchestrator },
  };
  return store;
}

let daemon;
beforeEach(() => {
  daemon = fakeDaemon();
  install(daemon);
});

test("entering Brainstorm starts a fresh session instead of showing the chat", async () => {
  const k = await kinds;
  daemon.sessions[0].messages.push({
    id: "old",
    role: "user",
    text: "old chat",
    ts: 1,
  });
  daemon.sessions[0].title = "old chat";
  const { thread, list } = await k.enterKind(REPO, "brainstorm");
  assert.notEqual(thread.id, "s1");
  assert.deepEqual(thread.messages, []);
  assert.deepEqual(
    k.sessionsOfKind(REPO, list, "brainstorm").map((s) => s.id),
    [thread.id],
  );
  assert.deepEqual(
    k.sessionsOfKind(REPO, list, "chat").map((s) => s.id),
    ["s1"],
  );
});

test("each view returns to its own last session", async () => {
  const k = await kinds;
  const brainstorm = (await k.enterKind(REPO, "brainstorm")).thread.id;
  const chat = (await k.enterKind(REPO, "chat")).thread;
  assert.equal(chat.id, "s1");
  assert.equal(daemon.current, "s1");
  const again = (await k.enterKind(REPO, "brainstorm")).thread;
  assert.equal(again.id, brainstorm);
  const newer = await k.newSession(REPO, "chat");
  await k.enterKind(REPO, "brainstorm");
  assert.equal((await k.enterKind(REPO, "chat")).thread.id, newer.id);
});

test("a second brainstorm is listed only under Brainstorm", async () => {
  const k = await kinds;
  await k.enterKind(REPO, "brainstorm");
  await k.newSession(REPO, "brainstorm");
  const list = await k.listSessions(REPO, "chat");
  assert.equal(k.sessionsOfKind(REPO, list, "brainstorm").length, 2);
  assert.equal(k.sessionsOfKind(REPO, list, "chat").length, 1);
});

test("the brainstorm role rides only on the first message and stays hidden", async () => {
  const k = await kinds;
  const { thread, list } = await k.enterKind(REPO, "brainstorm");
  await k.sendInKind(REPO, "brainstorm", "Remote hosts over SSH", thread, list);
  const sent = daemon.calls.filter((c) => c.method === "chat.send");
  assert.ok(sent[0].params.text.startsWith("Remote hosts over SSH\n\n"));
  assert.ok(sent[0].params.text.includes(k.BRAINSTORM_MARKER));
  assert.equal(k.visibleText(sent[0].params.text), "Remote hosts over SSH");
  const title = daemon.sessions.find((s) => s.id === thread.id).title;
  assert.equal(k.visibleTitle(title), "Remote hosts over SSH");

  const later = await (await kinds).enterKind(REPO, "brainstorm");
  await k.sendInKind(
    REPO,
    "brainstorm",
    "Any SSH host",
    later.thread,
    later.list,
  );
  const second = daemon.calls.filter((c) => c.method === "chat.send")[1];
  assert.equal(second.params.text, "Any SSH host");
});

test("a chat's first message carries no brainstorm role", async () => {
  const k = await kinds;
  const { thread, list } = await k.enterKind(REPO, "chat");
  await k.sendInKind(REPO, "chat", "What is running?", thread, list);
  const sent = daemon.calls.find((c) => c.method === "chat.send");
  assert.equal(sent.params.text, "What is running?");
});

test("switching is refused while another session answers", async () => {
  const k = await kinds;
  await k.enterKind(REPO, "brainstorm");
  daemon.sessions.find((s) => s.id === daemon.current).busy = true;
  await assert.rejects(k.enterKind(REPO, "chat"), /still answering/);
});

test("when orchd reports kinds they win and no role is prepended", async () => {
  daemon = fakeDaemon({ withKinds: true });
  const store = install(daemon);
  const k = await kinds;
  const { thread, list } = await k.enterKind(REPO, "brainstorm");
  assert.equal(
    daemon.calls.find((c) => c.method === "chat.new").params.kind,
    "brainstorm",
  );
  assert.equal(k.kindOf(REPO, { id: "s1", busy: false, kind: "chat" }), "chat");
  await k.sendInKind(REPO, "brainstorm", "Idea", thread, list);
  assert.equal(
    daemon.calls.find((c) => c.method === "chat.send").params.text,
    "Idea",
  );
  assert.ok(store.size >= 1);
});

test("storage that throws leaves kinds working in memory-free mode", async () => {
  const k = await kinds;
  globalThis.window.localStorage = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
  };
  const { thread } = await k.enterKind(REPO, "chat");
  assert.equal(thread.id, "s1");
});

test("session meta prefers orchd's fields, then what the app saw", async () => {
  const k = await kinds;
  k.noteThread(REPO, {
    repo: REPO,
    id: "s1",
    busy: false,
    messages: [
      { id: "a", role: "assistant", text: "It  failed\nverify", ts: 5 },
    ],
  });
  assert.deepEqual(k.sessionMeta(REPO, { id: "s1", busy: false }), {
    time: 5,
    preview: "It failed verify",
  });
  assert.deepEqual(
    k.sessionMeta(REPO, { id: "s1", busy: false, updatedAt: 9, preview: "p" }),
    { time: 9, preview: "p" },
  );
  assert.deepEqual(k.sessionMeta(REPO, { id: "s9", busy: false }), {
    time: undefined,
    preview: undefined,
  });
});

test("sessions group newest first into TODAY and EARLIER", async () => {
  const { groupSessions } = await model;
  const now = new Date(2026, 8, 29, 12).getTime();
  const rows = [
    { session: { id: "old", title: "Weekly" }, time: now - 3 * 86_400_000 },
    { session: { id: "unknown", title: "Seen long ago" } },
    { session: { id: "a", title: "A" }, time: now - 3_600_000 },
    { session: { id: "fresh" } },
  ];
  const groups = groupSessions(rows, now);
  assert.deepEqual(
    groups.map((g) => [g.label, g.rows.map((r) => r.session.id)]),
    [
      ["TODAY", ["fresh", "a"]],
      ["EARLIER", ["unknown", "old"]],
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

test("options come from the last sushi-options block and are stripped", async () => {
  const { optionsInText, stripOptions } = await model;
  const text =
    'Which hosts?\n```sushi-options\n["Only lab", "Any SSH host", 3]\n```';
  assert.deepEqual(optionsInText(text), ["Only lab", "Any SSH host"]);
  assert.equal(stripOptions(text), "Which hosts?");
  assert.deepEqual(optionsInText("```sushi-options\n[oops\n```"), []);
});

test("search matches title or preview, case-insensitively", async () => {
  const { matchesQuery } = await model;
  assert.ok(matchesQuery("", "x"));
  assert.ok(matchesQuery("EXPORT", "Why did export fail?", undefined));
  assert.ok(!matchesQuery("spend", "Why did export fail?", "It failed"));
});
