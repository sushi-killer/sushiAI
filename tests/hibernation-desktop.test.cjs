const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/daemonSessions.ts");
const attention = import("../src/app/attention.ts");
const { registerDaemonIpc } = require("../electron/ipc/daemon.cjs");

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
  agent: "claude",
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
const ready = (sessions) => ({
  ready: true,
  listed: true,
  sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
});
const state = (host, capabilities, extra = {}) => ({
  host,
  state: "ready",
  generation: 1,
  capabilities,
  ...extra,
});

test("a hibernated session is a live sleeping panel, a waking one is waking, neither is ended", async () => {
  const { reconcileSessions } = await library;
  const before = [workspace([panel("p1", "s1", { status: "idle" })])];
  const asleep = reconcileSessions(before, {
    local: ready([
      session("s1", {
        status: "hibernated",
        agentStatus: "idle",
        hibernatedAt: 1,
        agentSession: "c1",
      }),
    ]),
  });
  const sleeping = asleep[0].panels[0];
  assert.equal(sleeping.status, "sleeping");
  assert.equal(sleeping.ended, undefined);
  assert.equal(sleeping.agentSession, "c1");
  const waking = reconcileSessions(asleep, {
    local: ready([session("s1", { status: "waking", incarnation: 1 })]),
  });
  assert.equal(waking[0].panels[0].status, "waking");
  assert.equal(waking[0].panels[0].ended, undefined);
  const running = reconcileSessions(waking, {
    local: ready([session("s1", { agentStatus: "idle" })]),
  });
  assert.equal(running[0].panels[0].status, "idle");
});

test("the panel follows the session's pin as keepAwake", async () => {
  const { reconcileSessions } = await library;
  const before = [workspace([panel("p1", "s1")])];
  const pinned = reconcileSessions(before, {
    local: ready([session("s1", { pinned: true })]),
  });
  assert.equal(pinned[0].panels[0].keepAwake, true);
  const unpinned = reconcileSessions(pinned, {
    local: ready([session("s1")]),
  });
  assert.equal(unpinned[0].panels[0].keepAwake, undefined);
});

test("a sleeping agent is idle in the Inbox, never needs attention and is not a shell", async () => {
  const { reconcileSessions } = await library;
  const { createAttentionState, inboxGroups, waitingCount, observe } =
    await attention;
  const asleep = reconcileSessions([workspace([panel("p1", "s1")])], {
    local: ready([
      session("s1", { status: "hibernated", agentStatus: "idle" }),
    ]),
  });
  const attentionState = createAttentionState();
  const { state: seen, events } = observe(attentionState, asleep, 1);
  assert.deepEqual(events, []);
  const groups = inboxGroups(asleep, seen, []);
  const idle = groups.find((group) => group.key === "idle");
  assert.deepEqual(
    idle.rows.map((row) => row.panel.id),
    ["p1"],
  );
  assert.equal(waitingCount(asleep, seen, []), 0);
});

test("only a ready host that reported hibernate supports it", async () => {
  const { hostSupports } = await library;
  const states = [
    state("local", ["hibernate"]),
    state("old", ["asks"]),
    state("down", ["hibernate"], { state: "offline" }),
  ];
  assert.equal(hostSupports(states, "local", "hibernate"), true);
  assert.equal(hostSupports(states, "old", "hibernate"), false);
  assert.equal(hostSupports(states, "down", "hibernate"), false);
  assert.equal(hostSupports(states, "missing", "hibernate"), false);
});

test("the settings choices map to seconds and the default is four hours", async () => {
  const { HIBERNATE_CHOICES, DEFAULT_HIBERNATE_SECS } = await library;
  assert.deepEqual(
    HIBERNATE_CHOICES.map((choice) => [choice.label, choice.secs]),
    [
      ["Off", 0],
      ["1 h", 3600],
      ["4 h", 14400],
      ["12 h", 43200],
    ],
  );
  assert.equal(DEFAULT_HIBERNATE_SECS, 14400);
});

test("configure goes only to ready hosts that report the capability", async () => {
  const { configureAllHosts, configureHibernation } = await library;
  const calls = [];
  const bridge = {
    daemonStates: async () => [
      state("local", ["hibernate"]),
      state("old", ["asks"]),
      state("down", ["hibernate"], { state: "offline" }),
      state("ssh-1", ["hibernate"]),
    ],
    daemonConfigure: async (host, secs) => calls.push([host, secs]),
  };
  await configureAllHosts(bridge, 3600);
  assert.deepEqual(calls, [
    ["local", 3600],
    ["ssh-1", 3600],
  ]);
  calls.length = 0;
  await configureHibernation(bridge, state("old", []), 0);
  assert.deepEqual(calls, []);
  await configureHibernation(bridge, state("local", ["hibernate"]), 0);
  assert.deepEqual(calls, [["local", 0]], "Off pushes 0");
});

test("Reopen wakes an agent in place; terminals and agents without a conversation create", async () => {
  const { wakeRequest } = await library;
  const owner = workspace([], "ssh:box");
  assert.deepEqual(
    wakeRequest(owner, panel("a", "s1", { agentSession: "c1" }), "local"),
    { host: "box", id: "s1" },
  );
  assert.equal(wakeRequest(owner, panel("a", "s1"), "local"), null);
  assert.equal(
    wakeRequest(
      owner,
      panel("t", "s1", { kind: "terminal", agentSession: "c1" }),
      "local",
    ),
    null,
  );
  assert.equal(
    wakeRequest(owner, panel("a", undefined, { agentSession: "c1" }), "local"),
    null,
  );
});

test("wakeInPlace falls back on 1003 and 1012, never without the capability, throws on other errors", async () => {
  const { wakeInPlace } = await library;
  const make = (capabilities, result) => {
    const wakes = [];
    return {
      wakes,
      bridge: {
        daemonStates: async () => [state("local", capabilities)],
        sessionWake: async (host, id) => {
          wakes.push([host, id]);
          return result;
        },
      },
    };
  };
  const request = { host: "local", id: "s1" };
  const ok = make(["hibernate"], { ok: true });
  assert.equal(await wakeInPlace(ok.bridge, request), true);
  assert.deepEqual(ok.wakes, [["local", "s1"]]);
  for (const code of [1012, 1003]) {
    const refused = make(["hibernate"], { ok: false, code, message: "no" });
    assert.equal(await wakeInPlace(refused.bridge, request), false, `${code}`);
  }
  const old = make([], { ok: true });
  assert.equal(await wakeInPlace(old.bridge, request), false);
  assert.deepEqual(old.wakes, [], "an old host is never asked to wake");
  const broken = make(["hibernate"], {
    ok: false,
    code: 1001,
    message: "boom",
  });
  await assert.rejects(wakeInPlace(broken.bridge, request), /boom/);
});

test("the wake, focus and configure channels send the contract's methods", async () => {
  const handlers = new Map();
  const requests = [];
  let failWith = null;
  registerDaemonIpc({
    handle: (channel, run) => handlers.set(channel, run),
    send: () => {},
    getManager: () => ({
      request: async (host, method, params) => {
        requests.push([host, method, params]);
        if (failWith) throw Object.assign(new Error("refused"), failWith);
        return {};
      },
    }),
    launch: async () => {},
    installHost: async () => {},
  });
  assert.deepEqual(await handlers.get("daemon-session-wake")("local", "s1"), {
    ok: true,
  });
  await handlers.get("daemon-session-focus")("local", "s1", true);
  await handlers.get("daemon-configure")("local", 3600);
  await handlers.get("daemon-session-update")("local", {
    id: "s1",
    pinned: true,
  });
  assert.deepEqual(requests, [
    ["local", "session.wake", { id: "s1" }],
    ["local", "session.focus", { id: "s1", focused: true }],
    ["local", "daemon.configure", { hibernateAfterSecs: 3600 }],
    [
      "local",
      "session.update",
      {
        id: "s1",
        project: undefined,
        group: undefined,
        title: undefined,
        pinned: true,
      },
    ],
  ]);
  failWith = { code: 1012 };
  assert.deepEqual(await handlers.get("daemon-session-wake")("local", "s1"), {
    ok: false,
    code: 1012,
    message: "refused",
  });
  for (const [channel, args] of [
    ["daemon-session-wake", ["local", ""]],
    ["daemon-session-focus", ["local", "s1", "yes"]],
    ["daemon-configure", ["local", -1]],
    ["daemon-configure", ["local", 1.5]],
    ["daemon-session-update", ["local", { id: "s1", pinned: "yes" }]],
  ])
    await assert.rejects(
      handlers.get(channel)(...args),
      (error) => /^Invalid /.test(error.message),
      channel,
    );
});

test("a wake from a click or a keystroke that the host refuses with 1012 or 1003 runs the Reopen fallback", async () => {
  const { wakeOrReopen } = await library;
  const request = { host: "local", id: "s1" };
  for (const code of [1012, 1003]) {
    let reopened = 0;
    const bridge = {
      sessionWake: async () => ({ ok: false, code, message: "no" }),
    };
    assert.equal(await wakeOrReopen(bridge, request, () => reopened++), null);
    assert.equal(reopened, 1, `${code} reopens`);
  }
  let reopened = 0;
  const ok = { sessionWake: async () => ({ ok: true }) };
  assert.equal(await wakeOrReopen(ok, request, () => reopened++), null);
  const broken = {
    sessionWake: async () => ({ ok: false, code: 1001, message: "boom" }),
  };
  assert.equal(await wakeOrReopen(broken, request, () => reopened++), "boom");
  assert.equal(reopened, 0, "only a woken or a refused session never reopens");
});

test("a new connection generation of the host re-reads the capability; others change nothing", async () => {
  const { hostGenerationChange } = await library;
  const ready = (generation, capabilities, extra = {}) =>
    state("local", capabilities, { generation, ...extra });
  assert.deepEqual(hostGenerationChange(-1, ready(1, ["hibernate"]), "local"), {
    generation: 1,
    canFocus: true,
  });
  assert.equal(hostGenerationChange(1, ready(1, ["hibernate"]), "local"), null);
  // A restart brings an older daemon: the capability is gone.
  assert.deepEqual(hostGenerationChange(1, ready(2, []), "local"), {
    generation: 2,
    canFocus: false,
  });
  assert.equal(hostGenerationChange(1, ready(2, []), "other"), null);
  assert.equal(
    hostGenerationChange(1, ready(2, [], { state: "connecting" }), "local"),
    null,
  );
});

test("Reopen applies to an ended, unstarted or sleeping pane, not to a running one", async () => {
  const { canReopen } = await library;
  const base = { kind: "agent", sessionId: "s1" };
  assert.equal(canReopen({ ...base, status: "sleeping" }), true);
  assert.equal(canReopen({ ...base, ended: true, status: "idle" }), true);
  assert.equal(canReopen({ kind: "terminal", status: "idle" }), true);
  assert.equal(canReopen({ ...base, status: "idle" }), false);
  assert.equal(canReopen({ ...base, status: "working" }), false);
  assert.equal(canReopen({ ...base, status: "waking" }), false);
  assert.equal(
    canReopen({ kind: "browser", sessionId: "s1", status: "sleeping" }),
    false,
  );
});

test("an unavailable launch key shows a notice and never falls back to a new session", async () => {
  const { wakeOrReopen, wakeInPlace } = await library;
  const request = { host: "local", id: "s1" };
  const refused = { ok: false, code: 1013, message: "key" };
  let reopened = 0;
  const notice = await wakeOrReopen(
    { sessionWake: async () => refused },
    request,
    () => reopened++,
  );
  assert.equal(notice, "Unlock the keychain to wake this agent");
  assert.equal(reopened, 0);
  await assert.rejects(
    wakeInPlace(
      {
        daemonStates: async () => [state("local", ["hibernate"])],
        sessionWake: async () => refused,
      },
      request,
    ),
    /Unlock the keychain/,
  );
});
