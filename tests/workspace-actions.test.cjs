const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/workspace/workspace-actions.ts");
const layoutLibrary = import("../src/layout.ts");

const panel = (id, kind = "terminal", extra = {}) => ({
  id,
  kind,
  title: id,
  ...extra,
});

async function workspace(panels, ids = panels.map((p) => p.id)) {
  const { leaf, split } = await layoutLibrary;
  const layout = ids.length
    ? ids
        .slice(1)
        .reduce((tree, id) => split(tree, leaf(id), "row", 0.5), leaf(ids[0]))
    : null;
  return { id: "w1", name: "sushiai", cwd: "/tmp", panels, layout };
}

test("appendPanel adds the panel and splits the existing layout", async () => {
  const { appendPanel } = await library;
  const { contains } = await layoutLibrary;
  const before = await workspace([panel("a")]);
  const after = appendPanel(before, panel("b"));
  assert.deepEqual(
    after.panels.map((item) => item.id),
    ["a", "b"],
  );
  assert.ok(contains(after.layout, "a") && contains(after.layout, "b"));
  assert.equal(after.layout.ratio, 0.65, "the existing layout keeps 65%");
  assert.equal(before.panels.length, 1, "the input workspace is untouched");

  const empty = appendPanel(await workspace([], []), panel("only"));
  assert.deepEqual(empty.layout, { type: "leaf", id: "only" });
});

test("removePanel drops a terminal but keeps Herdr panes and chat threads", async () => {
  const { removePanel } = await library;
  const { contains } = await layoutLibrary;
  const terminal = panel("t");
  const pane = panel("p", "terminal", { herdrId: "pane-1" });
  const thread = panel("c", "chat", {
    messages: [{ role: "user", text: "hi" }],
  });
  const before = await workspace([terminal, pane, thread]);

  const withoutTerminal = removePanel(before, terminal);
  assert.equal(
    withoutTerminal.panels.some((item) => item.id === "t"),
    false,
    "a plain terminal is gone for good",
  );
  assert.equal(contains(withoutTerminal.layout, "t"), false);

  const withoutPane = removePanel(before, pane);
  assert.ok(
    withoutPane.panels.some((item) => item.id === "p"),
    "a Herdr pane survives: closing the view never kills the session",
  );
  assert.equal(contains(withoutPane.layout, "p"), false);

  const withoutThread = removePanel(before, thread);
  assert.ok(
    withoutThread.panels.some((item) => item.id === "c"),
    "a chat thread keeps its transcript and only leaves the layout",
  );
  assert.equal(contains(withoutThread.layout, "c"), false);
});

test("removeClosedPanels applies to the workspace list as it is now", async () => {
  const { removeClosedPanels } = await library;
  const { contains } = await layoutLibrary;
  const first = await workspace([panel("a"), panel("b")]);
  const second = {
    ...(await workspace([panel("c"), panel("d", "chat")])),
    id: "w2",
  };
  const list = [first, second];

  const same = removeClosedPanels(list, new Set());
  assert.equal(same, list, "no closures means the same array, so no re-render");

  const next = removeClosedPanels(list, new Set(["b", "d"]));
  assert.deepEqual(
    next[0].panels.map((item) => item.id),
    ["a"],
  );
  assert.deepEqual(
    next[1].panels.map((item) => item.id),
    ["c", "d"],
    "a closed chat thread keeps its transcript",
  );
  assert.equal(contains(next[1].layout, "d"), false, "but leaves the layout");
  assert.equal(next[0].panels.length, 1);
  assert.equal(first.panels.length, 2, "the input list is untouched");

  assert.equal(
    removeClosedPanels(list, new Set(["nothing-here"])),
    list,
    "ids from a workspace the user already left change nothing",
  );
});

test("movePanel swaps on a center drop and re-inserts on an edge drop", async () => {
  const { movePanel } = await library;
  const { contains } = await layoutLibrary;
  const before = await workspace([panel("a"), panel("b"), panel("c")]);

  const swapped = movePanel(before.layout, "a", "c", "center");
  assert.ok(contains(swapped, "a") && contains(swapped, "c"));
  assert.notDeepEqual(swapped, before.layout);

  const moved = movePanel(before.layout, "a", "c", "right");
  assert.ok(contains(moved, "a") && contains(moved, "c"));

  assert.equal(
    movePanel(before.layout, "a", "missing", "right"),
    before.layout,
    "a target that is no longer in the layout is a no-op",
  );
  assert.equal(
    movePanel(before.layout, "missing", "a", "right"),
    before.layout,
    "a source that is no longer in the layout is a no-op",
  );
  assert.equal(movePanel(null, "a", "b", "right"), null);
});

test("tidyWorkspace arranges the code panels and drops detached threads", async () => {
  const { tidyWorkspace } = await library;
  const { contains } = await layoutLibrary;
  const detached = panel("thread", "chat");
  const before = {
    ...(await workspace([panel("a"), panel("b")])),
  };
  before.panels = [...before.panels, detached];
  const after = tidyWorkspace(before);
  assert.ok(contains(after.layout, "a") && contains(after.layout, "b"));
  assert.equal(
    contains(after.layout, "thread"),
    false,
    "a thread that was never in the layout is not pulled into it",
  );
});

test("fixSelection follows panels that disappeared", async () => {
  const { fixSelection } = await library;
  const current = await workspace([panel("a"), panel("b")]);
  assert.deepEqual(fixSelection(current, "b", "b"), {
    selected: "b",
    zoomed: "b",
  });
  assert.deepEqual(
    fixSelection(current, "gone", "gone"),
    { selected: "a", zoomed: null },
    "selection falls back to the first panel, zoom simply clears",
  );
  const empty = await workspace([], []);
  assert.deepEqual(fixSelection(empty, "gone", null), {
    selected: "",
    zoomed: null,
  });
});

test("blockedPanels pairs waiting panels with their workspace", async () => {
  const { blockedPanels } = await library;
  const first = await workspace([
    panel("idle"),
    panel("waiting", "agent", { status: "blocked" }),
  ]);
  const second = {
    ...(await workspace([panel("also", "agent", { status: "blocked" })])),
    id: "w2",
  };
  const found = blockedPanels([first, second]);
  assert.deepEqual(
    found.map((item) => [item.workspace.id, item.panel.id]),
    [
      ["w1", "waiting"],
      ["w2", "also"],
    ],
  );
  assert.deepEqual(blockedPanels([await workspace([panel("idle")])]), []);
});

test("retitleTerminal follows the agent but never overwrites a typed name", async () => {
  const { retitleTerminal } = await library;
  const fresh = panel("t", "terminal");
  fresh.title = "zsh";
  assert.equal(retitleTerminal(fresh, "claude").title, "Claude Code");
  assert.equal(retitleTerminal(fresh, "claude").agent, "claude");

  const running = { ...fresh, title: "Claude Code", agent: "claude" };
  assert.equal(
    retitleTerminal(running, null).title,
    "zsh",
    "the agent exiting restores the shell name",
  );
  assert.equal(retitleTerminal(running, null).agent, undefined);

  const named = { ...fresh, title: "build server" };
  assert.equal(
    retitleTerminal(named, "codex").title,
    "build server",
    "a name the user typed survives",
  );

  const chat = { ...fresh, kind: "chat", title: "zsh" };
  assert.equal(
    retitleTerminal(chat, "claude").title,
    "zsh",
    "only terminals follow the agent",
  );
});

const profile = (id, name) => ({
  id,
  name,
  host: "192.0.2.10",
  socket: `ssh:${id}`,
});

/** A synthetic two-member merge group (Local + a Lab SSH host), plus a
 * distractor workspace that is deliberately left out of it. */
async function mergeGroupFixture() {
  const local = {
    ...(await workspace([panel("a"), panel("b")])),
    id: "w-local",
    cwd: "/Users/dev/app",
  };
  const lab = {
    ...(await workspace([panel("c")])),
    id: "w-lab",
    cwd: "/home/dev/app",
    connection: "ssh:lab",
  };
  const other = {
    ...(await workspace([panel("distractor")])),
    id: "w-other",
    cwd: "/Users/dev/unrelated",
  };
  const git = {
    remote: "example.test/dev/app",
    commonDir: "",
    checkout: "",
    subdir: "",
    branch: "",
  };
  const group = {
    id: "example.test/dev/app::",
    worktrees: false,
    members: [
      { workspace: local, hostKey: "local", git },
      { workspace: lab, hostKey: "ssh:lab", git },
    ],
  };
  return { group, local, lab, other };
}

test("groupPanelIds and tidyGroupLayout combine every member's code panels, never a workspace outside the group", async () => {
  const { groupPanelIds, tidyGroupLayout } = await library;
  const { contains } = await layoutLibrary;
  const { group } = await mergeGroupFixture();
  assert.deepEqual(groupPanelIds(group), ["a", "b", "c"]);
  const tidied = tidyGroupLayout(group);
  assert.ok(
    contains(tidied, "a") && contains(tidied, "b") && contains(tidied, "c"),
  );
  assert.equal(contains(tidied, "distractor"), false);
});

test("resolveGroupPanes resolves each pane to its own owner's cwd, endpoint and host label", async () => {
  const { resolveGroupPanes } = await library;
  const { group } = await mergeGroupFixture();
  const panes = resolveGroupPanes(
    group,
    [profile("lab", "Lab")],
    "local-socket",
  );
  assert.deepEqual(
    panes.map((p) => p.panel.id),
    ["a", "b", "c"],
  );
  const [a, , c] = panes;
  assert.equal(a.cwd, "/Users/dev/app");
  assert.equal(
    a.socket,
    "local-socket",
    "the Local member falls back to the app socket",
  );
  assert.equal(a.endpoint, undefined);
  assert.equal(a.hostLabel, "Local");
  assert.equal(c.cwd, "/home/dev/app");
  assert.equal(c.socket, "ssh:lab", "the Lab member uses its own connection");
  assert.equal(c.endpoint, "ssh:lab");
  assert.equal(c.hostLabel, "Lab");
});

test("reconcileGroupLayout drops panels a Herdr poll removed and appends new ones, without touching a workspace outside the group", async () => {
  const { reconcileGroupLayout } = await library;
  const { tidy, contains, leafIds } = await layoutLibrary;
  const first = reconcileGroupLayout(undefined, ["a", "b", "c"]);
  assert.deepEqual(
    leafIds(first).sort(),
    ["a", "b", "c"],
    "nothing stored yet falls back to tidy",
  );

  const stored = tidy(["a", "b", "c"]);
  const reconciled = reconcileGroupLayout(stored, ["a", "c", "d"]);
  assert.ok(contains(reconciled, "a"));
  assert.ok(contains(reconciled, "c"));
  assert.ok(contains(reconciled, "d"), "a pane a member gained is appended");
  assert.equal(
    contains(reconciled, "b"),
    false,
    "a pane a member lost leaves the layout",
  );
  assert.equal(contains(reconciled, "distractor"), false);
});

test("fixSelection widens aliveness across an active merge group's members, but never to a workspace outside it", async () => {
  const { fixSelection } = await library;
  const { local } = await mergeGroupFixture();
  const groupIds = ["a", "b", "c"];

  assert.deepEqual(
    fixSelection(local, "c", "c", groupIds),
    { selected: "c", zoomed: "c" },
    "a pane owned by a different member of the same group stays selected",
  );
  assert.deepEqual(
    fixSelection(local, "distractor", null, groupIds),
    { selected: "a", zoomed: null },
    "a pane from a workspace outside the group is not alive",
  );
  assert.deepEqual(
    fixSelection(local, "c", "c"),
    { selected: "a", zoomed: null },
    "without a group, cross-workspace ids fall back exactly like today (C3)",
  );
});
