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
  // Unseen only through a real transition: a panel already finished when the
  // app started was seen before the restart.
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "working" })])],
    0,
  ));
  ({ state } = observe(
    state,
    [workspace("w1", [panel("a", "agent", { status: "done" })])],
    1000,
  ));
  assert.ok(state.panels.a);
  assert.ok(state.unseen.has("a"));
  ({ state } = observe(state, [workspace("w1", [])], 2000));
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
  // "c" has to finish while the app is watching to count as unseen.
  ({ state } = observe(
    state,
    [ws[0], workspace("w2", [panel("c", "agent", { status: "working" })])],
    0,
  ));
  ({ state } = observe(state, ws, 1000));
  assert.equal(waitingCount(ws, state, []), 2);
  state = markSeen(state, "c");
  assert.equal(waitingCount(ws, state, []), 1);
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

test("waitingCount skips hosts hidden from the sidebar, like the Inbox does", async () => {
  const { createAttentionState, observe, waitingCount } = await library;
  const hidden = {
    id: "w-lab",
    name: "app",
    cwd: "/srv/app",
    connection: "ssh:lab",
    panels: [
      { id: "p-lab", kind: "agent", title: "Claude Code", status: "blocked" },
    ],
    layout: null,
  };
  const { state } = observe(createAttentionState(), [hidden], 0);
  assert.equal(waitingCount([hidden], state, []), 1, "visible by default");
  assert.equal(
    waitingCount([hidden], state, [
      {
        id: "lab",
        name: "Lab",
        host: "192.0.2.10",
        socket: "ssh:lab",
        hidden: true,
      },
    ]),
    0,
    "a hidden host contributes nothing to the badge",
  );
});

test("a session already finished when the app starts is not counted as unseen", async () => {
  const { createAttentionState, observe, waitingCount, inboxGroups } =
    await library;
  const ws = [
    workspace("w1", [
      panel("a", "agent", { status: "done" }),
      panel("b", "agent", { status: "blocked" }),
    ]),
  ];
  const first = observe(createAttentionState(), ws, 0);
  assert.deepEqual(first.events, [], "startup is silent");
  assert.equal(
    waitingCount(ws, first.state, []),
    1,
    "only the blocked agent waits; the finished one was already seen",
  );
  const groups = inboxGroups(ws, first.state, []);
  const byKey = Object.fromEntries(groups.map((g) => [g.key, g.rows.length]));
  assert.equal(byKey.done, 0, "nothing lands in Done - not seen at startup");
  assert.equal(byKey.idle, 1, "the finished agent sits in Idle instead");
  const later = observe(
    first.state,
    [workspace("w1", [panel("a", "agent", { status: "working" })])],
    1000,
  );
  const back = observe(
    later.state,
    [workspace("w1", [panel("a", "agent", { status: "done" })])],
    2000,
  );
  assert.equal(back.events.length, 1, "finishing while the app runs is news");
  assert.equal(
    inboxGroups(
      [workspace("w1", [panel("a", "agent", { status: "done" })])],
      back.state,
      [],
    ).find((g) => g.key === "done").rows.length,
    1,
  );
});

test("only sessions are listed: a browser, files or chat panel never reaches the Inbox", async () => {
  const { createAttentionState, inboxGroups, waitingCount } = await library;
  const ws = [
    workspace("w1", [
      panel("term", "terminal", {}),
      panel("web", "browser", { status: "done" }),
      panel("files", "files", {}),
      panel("thread", "chat", {}),
    ]),
  ];
  const listed = inboxGroups(ws, createAttentionState(), [])
    .flatMap((group) => group.rows)
    .map((row) => row.panel.id);
  assert.deepEqual(listed, ["term"]);
  assert.equal(waitingCount(ws, createAttentionState(), []), 0);
});

test("a terminal with an agent inside is timed, notified and reminded like an agent panel", async () => {
  const { createAttentionState, observe, dueReminders, inboxGroups } =
    await library;
  const shell = (status) =>
    workspace("w1", [panel("t", "terminal", { status, agent: "claude" })]);
  let { state } = observe(createAttentionState(), [shell("working")], 0);
  const seen = observe(state, [shell("blocked")], 60000);
  state = seen.state;
  assert.equal(seen.events.length, 1, "blocked fires for a detected agent");
  assert.equal(seen.events[0].kind, "blocked");
  const rows = inboxGroups([shell("blocked")], state, []).find(
    (group) => group.key === "blocked",
  ).rows;
  assert.equal(rows.length, 1);
  assert.equal(rows[0].since, 60000, "it is timed like any other session");
  const due = dueReminders(state, 60000 + 5 * 60000);
  assert.deepEqual(due.reminders, [{ panelId: "t", minutes: 5 }]);
});

test("workingCount counts running sessions and skips hidden hosts", async () => {
  const { createAttentionState, observe, workingCount } = await library;
  const ws = [
    workspace("w1", [
      panel("a", "agent", { status: "working" }),
      panel("b", "agent", { status: "blocked" }),
      panel("c", "terminal"),
    ]),
    workspace("w2", [panel("d", "agent", { status: "working" })], {
      connection: "ssh:lab",
    }),
  ];
  const { state } = observe(createAttentionState(), ws, 0);
  assert.equal(workingCount(ws, state, []), 2, "both hosts visible");
  assert.equal(
    workingCount(ws, state, [
      {
        id: "lab",
        name: "user@devbox",
        host: "192.0.2.10",
        socket: "ssh:lab",
        hidden: true,
      },
    ]),
    1,
    "a hidden host contributes nothing to the menu bar mark",
  );
});
