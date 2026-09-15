const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/attention.ts");

const MIN = 60000;

function panel(id, kind = "agent", extra = {}) {
  return { id, kind, title: id, ...extra };
}
function workspace(id, panels, extra = {}) {
  return { id, name: id, cwd: "/tmp", panels, layout: null, ...extra };
}

test("the first observation of a panel is silent", async () => {
  const { createAttentionState, observe } = await library;
  const ws = [workspace("w1", [panel("a", "agent", { status: "blocked" })])];
  const { state, events } = observe(createAttentionState(), ws, 1000);
  assert.deepEqual(events, []);
  assert.equal(state.panels.a.status, "blocked");
  assert.equal(state.panels.a.since, 1000);
});

test("working to blocked fires one event; a distractor panel in another workspace stays working", async () => {
  const { createAttentionState, observe } = await library;
  let state = createAttentionState();
  ({ state } = observe(
    state,
    [
      workspace("w1", [panel("a", "agent", { status: "working" })]),
      workspace("w2", [panel("b", "agent", { status: "working" })]),
    ],
    0,
  ));
  const { events } = observe(
    state,
    [
      workspace("w1", [panel("a", "agent", { status: "blocked" })]),
      workspace("w2", [panel("b", "agent", { status: "working" })]),
    ],
    1000,
  );
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, "blocked");
  assert.equal(events[0].panel.id, "a");
  assert.equal(events[0].workspace.id, "w1");
});

test("a panel entering done becomes unseen until markSeen", async () => {
  const { createAttentionState, observe, markSeen } = await library;
  let state = createAttentionState();
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "working" })])],
    0,
  ));
  const { state: state2, events } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "done" })])],
    1000,
  );
  assert.equal(events.length, 1);
  assert.equal(events[0].kind, "done");
  assert.ok(state2.unseen.has("a"));
  const state3 = markSeen(state2, "a");
  assert.ok(!state3.unseen.has("a"));
  // markSeen on a panel that was never unseen is a no-op, not an error.
  assert.equal(markSeen(state3, "a"), state3);
});

test("reminders fire once each at 5, 10 and 20 minutes, never at 4:59", async () => {
  const { createAttentionState, observe, dueReminders } = await library;
  let state = createAttentionState();
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "blocked" })])],
    0,
  ));
  let result = dueReminders(state, 4 * MIN + 59000);
  assert.deepEqual(
    result.reminders,
    [],
    "4:59 must not fire the 5 minute mark",
  );

  result = dueReminders(state, 5 * MIN);
  assert.deepEqual(result.reminders, [{ panelId: "a", minutes: 5 }]);
  state = result.state;

  // Still under 10 minutes, and 5 already fired - nothing due.
  result = dueReminders(state, 5 * MIN + 1000);
  assert.deepEqual(result.reminders, []);
  state = result.state;

  result = dueReminders(state, 10 * MIN);
  assert.deepEqual(result.reminders, [{ panelId: "a", minutes: 10 }]);
  state = result.state;

  result = dueReminders(state, 20 * MIN);
  assert.deepEqual(result.reminders, [{ panelId: "a", minutes: 20 }]);
  state = result.state;

  // Every mark has fired; nothing left to remind, ever.
  result = dueReminders(state, 60 * MIN);
  assert.deepEqual(result.reminders, []);
});

test("leaving blocked and re-entering restarts the reminder schedule", async () => {
  const { createAttentionState, observe, dueReminders } = await library;
  let state = createAttentionState();
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "blocked" })])],
    0,
  ));
  ({ state } = dueReminders(state, 5 * MIN));
  // Leaves blocked - the 5 minute mark it already fired must not survive.
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "working" })])],
    6 * MIN,
  ));
  // Re-enters blocked.
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "blocked" })])],
    7 * MIN,
  ));
  const result = dueReminders(state, 7 * MIN + 5 * MIN);
  assert.deepEqual(result.reminders, [{ panelId: "a", minutes: 5 }]);
});

test("a panel that disappears - closed, or its workspace closed - is dropped", async () => {
  const { createAttentionState, observe } = await library;
  let state = createAttentionState();
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "done" })])],
    0,
  ));
  assert.ok(state.panels.a);
  assert.ok(state.unseen.has("a"));
  ({ state } = observe(state, [workspace("w1", [])], 1000));
  assert.equal(state.panels.a, undefined);
  assert.ok(
    !state.unseen.has("a"),
    "an unseen panel that disappeared is no longer waiting on anything",
  );
});

test("waitingCount counts blocked and done-not-seen panels across every workspace", async () => {
  const { createAttentionState, observe, waitingCount, markSeen } =
    await library;
  let state = createAttentionState();
  const ws = [
    workspace("w1", [
      panel("a", "agent", { status: "blocked" }),
      panel("b", "agent", { status: "working" }),
    ]),
    workspace("w2", [panel("c", "agent", { status: "done" })]),
  ];
  ({ state } = observe(state, ws, 0));
  assert.equal(waitingCount(ws, state), 2);
  state = markSeen(state, "c");
  assert.equal(waitingCount(ws, state), 1);
});

test("cleanupSelection checks idle shells and seen idle/finished agents, never blocked or working", async () => {
  const { cleanupSelection } = await library;
  const w = workspace("w1", []);
  const rows = [
    { workspace: w, panel: panel("a"), group: "blocked", since: null },
    { workspace: w, panel: panel("b"), group: "working", since: null },
    { workspace: w, panel: panel("c"), group: "idle", since: null },
    { workspace: w, panel: panel("d"), group: "shells", since: null },
    { workspace: w, panel: panel("e"), group: "done", since: null },
  ];
  assert.deepEqual(cleanupSelection(rows), ["c", "d"]);
});

test("inboxGroups excludes a workspace whose host is hidden from the sidebar", async () => {
  const { createAttentionState, observe, inboxGroups } = await library;
  let state = createAttentionState();
  const hiddenProfile = {
    id: "p1",
    name: "user@devbox",
    host: "192.0.2.10",
    socket: "",
    hidden: true,
  };
  const ws = [
    workspace("w1", [panel("a", "terminal", {})]),
    {
      ...workspace("w2", [panel("b", "terminal", {})]),
      connection: "ssh:p1",
    },
  ];
  ({ state } = observe(state, ws, 0));
  const groups = inboxGroups(ws, state, [hiddenProfile]);
  assert.deepEqual(
    groups.flatMap((group) => group.rows.map((row) => row.panel.id)),
    ["a"],
  );
});
