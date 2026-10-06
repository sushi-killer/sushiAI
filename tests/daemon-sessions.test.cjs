const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/daemonSessions.ts");

const session = (id, extra = {}) => ({
  id,
  cmd: ["claude"],
  cwd: "/work/app",
  status: "running",
  cols: 80,
  rows: 24,
  ...extra,
});
const panel = (id, sessionId, extra = {}) => ({
  id,
  kind: "agent",
  title: id,
  sessionId,
  ...extra,
});
const workspace = (panels, connection = "local-socket") => ({
  id: "w1",
  name: "app",
  cwd: "/work/app",
  connection,
  panels,
  layout: null,
});
const ready = (sessions, extra = {}) => ({
  ready: true,
  listed: true,
  sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
  ...extra,
});

test("a known panel takes the session's status, title and folder", async () => {
  const { reconcileSessions } = await library;
  const before = [workspace([panel("p1", "s1", { title: "old" })])];
  for (const [agentStatus, status] of [
    ["starting", "working"],
    ["working", "working"],
    ["blocked", "blocked"],
    ["idle", "idle"],
  ]) {
    const after = reconcileSessions(before, {
      local: ready([
        session("s1", { agentStatus, title: "Build", cwd: "/work/app/sub" }),
      ]),
    });
    const next = after[0].panels[0];
    assert.equal(next.status, status, agentStatus);
    assert.equal(next.title, "Build");
    assert.equal(next.paneCwd, "/work/app/sub");
    assert.equal(next.ended, undefined);
  }
  assert.equal(before[0].panels[0].title, "old", "the input is untouched");
});

test("an agent turn that finished reads done until it is seen", async () => {
  const { reconcileSessions } = await library;
  const idle = { local: ready([session("s1", { agentStatus: "idle" })]) };
  const fromWorking = reconcileSessions(
    [workspace([panel("p1", "s1", { status: "working" })])],
    idle,
  );
  assert.equal(fromWorking[0].panels[0].status, "done");
  const fresh = reconcileSessions([workspace([panel("p1", "s1")])], idle);
  assert.equal(fresh[0].panels[0].status, "idle");
  const terminal = reconcileSessions(
    [workspace([panel("p1", "s1", { status: "working" })])],
    { local: ready([session("s1", { cmd: ["zsh"] })]) },
  );
  assert.equal(terminal[0].panels[0].status, undefined, "a shell has none");
});

test("an exited session ends its panel and keeps the agent session for Reopen", async () => {
  const { reconcileSessions } = await library;
  const after = reconcileSessions(
    [workspace([panel("p1", "s1", { status: "working" })])],
    {
      local: ready([
        session("s1", {
          status: "exited",
          exitCode: 0,
          agentStatus: "exited",
          agentSession: "thread-1",
        }),
      ]),
    },
  );
  const ended = after[0].panels[0];
  assert.equal(ended.ended, true);
  assert.equal(ended.status, undefined);
  assert.equal(ended.agentSession, "thread-1");
  assert.equal(ended.sessionId, "s1");
});

test("a detached session stays live", async () => {
  const { reconcileSessions } = await library;
  const after = reconcileSessions(
    [workspace([panel("p1", "s1", { ended: true })])],
    { local: ready([session("s1", { status: "detached" })]) },
  );
  assert.equal(after[0].panels[0].ended, undefined);
});

test("a session missing from the full list ends its panel", async () => {
  const { reconcileSessions } = await library;
  const after = reconcileSessions(
    [workspace([panel("p1", "s1", { agentSession: "keep" })])],
    { local: ready([session("other")]) },
  );
  assert.equal(after[0].panels[0].ended, true);
  assert.equal(after[0].panels[0].agentSession, "keep");
});

test("nothing ends while a host is not ready or not yet listed", async () => {
  const { reconcileSessions } = await library;
  const before = [workspace([panel("p1", "s1")])];
  for (const hosts of [
    {},
    { local: ready([], { ready: false }) },
    { local: ready([], { listed: false }) },
    { other: ready([]) },
  ])
    assert.equal(reconcileSessions(before, hosts), before);
});

test("each workspace follows its own host", async () => {
  const { reconcileSessions } = await library;
  const local = workspace([panel("a", "s1", { paneCwd: "/work/app" })]);
  const remote = { ...workspace([panel("b", "s1")], "ssh:devbox"), id: "w2" };
  const after = reconcileSessions([local, remote], {
    local: ready([session("s1")]),
    devbox: ready([]),
  });
  assert.equal(after[0], local, "a live session changes nothing");
  assert.equal(after[1].panels[0].ended, true);
});

test("a session the desktop did not create is never adopted", async () => {
  const { reconcileSessions } = await library;
  const before = [
    workspace([
      panel("p1", "s1", { paneCwd: "/work/app" }),
      { id: "t", kind: "terminal" },
    ]),
  ];
  const after = reconcileSessions(before, {
    local: ready([session("s1"), session("foreign", { group: "w1" })]),
  });
  assert.equal(after[0].panels.length, 2);
  assert.equal(after.length, 1);
  assert.equal(
    reconcileSessions(before, { local: ready([session("s1")]) }),
    before,
  );
});

test("events update a host's sessions incrementally", async () => {
  const { applyDaemonEvent, applySessionList, emptyHost } = await library;
  let host = applySessionList(emptyHost(), [session("s1")]);
  assert.equal(host.listed, true);
  host = applyDaemonEvent(host, {
    method: "session.created",
    params: session("s2", { agentStatus: "starting" }),
  });
  host = applyDaemonEvent(host, {
    method: "session.status",
    params: {
      id: "s2",
      status: "working",
      statusSource: "hook",
      statusSince: 5,
    },
  });
  assert.equal(host.sessions.s2.agentStatus, "working");
  host = applyDaemonEvent(host, {
    method: "session.meta",
    params: { id: "s2", agentSession: "thread", transcriptPath: "/t.jsonl" },
  });
  assert.equal(host.sessions.s2.agentSession, "thread");
  host = applyDaemonEvent(host, {
    method: "session.updated",
    params: { ...host.sessions.s2, title: "Renamed" },
  });
  assert.equal(host.sessions.s2.title, "Renamed");
  host = applyDaemonEvent(host, {
    method: "session.exited",
    params: { id: "s2", code: 3 },
  });
  assert.equal(host.sessions.s2.status, "exited");
  assert.equal(host.sessions.s2.exitCode, 3);
  assert.equal(host.sessions.s2.agentStatus, "exited");
  host = applyDaemonEvent(host, {
    method: "session.removed",
    params: { id: "s1" },
  });
  assert.deepEqual(Object.keys(host.sessions), ["s2"]);
  const same = applyDaemonEvent(host, {
    method: "session.removed",
    params: { id: "gone" },
  });
  assert.equal(same, host, "an unknown id changes nothing");
  assert.equal(
    applyDaemonEvent(host, { method: "session.ask", params: {} }),
    host,
  );
});

test("a removed session ends its panel once the host is listed", async () => {
  const { reconcileSessions, applyDaemonEvent, applySessionList, emptyHost } =
    await library;
  let host = {
    ...applySessionList(emptyHost(), [session("s1")]),
    ready: true,
  };
  const before = [workspace([panel("p1", "s1")])];
  const live = reconcileSessions(before, { local: host });
  host = applyDaemonEvent(host, {
    method: "session.removed",
    params: { id: "s1" },
  });
  const after = reconcileSessions(live, { local: host });
  assert.equal(after[0].panels[0].ended, true);
});

test("the daemon host of a connection and the panel id of a session", async () => {
  const { daemonHost, sessionPanelId } = await library;
  assert.equal(daemonHost(undefined), "local");
  assert.equal(daemonHost("/tmp/some.sock"), "local");
  assert.equal(daemonHost("ssh:devbox"), "devbox");
  assert.notEqual(sessionPanelId("ssh:a", "s1"), sessionPanelId("ssh:b", "s1"));
});
