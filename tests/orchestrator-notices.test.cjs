const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  orchestratorTarget,
  toastReducer,
  toastLifetimeMs,
  MAX_TOASTS,
} = require("../src/orchestrator/notices.ts");
const {
  publishReveal,
  pendingReveal,
  REVEAL_TTL_MS,
  subscribeReveal,
  resetReveal,
} = require("../src/orchestrator/reveal.ts");

const leaf = (id) => ({ type: "leaf", id });
const workspace = (id, cwd, panels = []) => ({
  id,
  name: id,
  cwd,
  panels,
  layout: panels.length ? leaf(panels[0].id) : null,
});

test("orchestratorTarget finds the repo's Orchestrator panel, else adds one, else creates the workspace", () => {
  const withPanel = workspace("w2", "/repo", [
    { id: "p1", kind: "terminal", title: "zsh" },
  ]);
  withPanel.panels.push({ id: "p2", kind: "orchestrator", title: "O" });
  withPanel.layout = {
    type: "split",
    id: "s",
    axis: "row",
    ratio: 0.5,
    a: leaf("p1"),
    b: leaf("p2"),
  };
  const plain = workspace("w1", "/repo", [
    { id: "p3", kind: "terminal", title: "zsh" },
  ]);
  const other = workspace("w3", "/other");
  assert.deepEqual(orchestratorTarget([other, plain, withPanel], "/repo"), {
    kind: "panel",
    workspaceId: "w2",
    panelId: "p2",
  });
  assert.deepEqual(orchestratorTarget([other, plain], "/repo"), {
    kind: "add-panel",
    workspaceId: "w1",
  });
  assert.deepEqual(orchestratorTarget([other], "/work/my-repo"), {
    kind: "create-workspace",
    name: "my-repo",
    cwd: "/work/my-repo",
  });
});

const notice = (kind, taskId, body = "b") => ({
  taskId,
  repo: "/repo",
  kind,
  title: "t",
  body,
  focus: kind === "input" ? "question" : "summary",
});

test("toastReducer caps at three, newest replacing the oldest, and dedupes", () => {
  let state = [];
  for (const id of ["a", "b", "c", "d"])
    state = toastReducer(state, { type: "add", notice: notice("done", id) });
  assert.equal(MAX_TOASTS, 3);
  assert.deepEqual(
    state.map((t) => t.taskId),
    ["b", "c", "d"],
  );
  state = toastReducer(state, { type: "add", notice: notice("done", "b") });
  assert.deepEqual(
    state.map((t) => t.taskId),
    ["c", "d", "b"],
  );
  state = toastReducer(state, { type: "dismiss", id: state[0].id });
  assert.deepEqual(
    state.map((t) => t.taskId),
    ["d", "b"],
  );
});

test("input toasts are sticky, done and failed fade, and a task leaving waiting drops its input toast", () => {
  assert.equal(toastLifetimeMs(notice("input", "a")), null);
  assert.equal(toastLifetimeMs(notice("done", "a")), 8000);
  assert.equal(toastLifetimeMs(notice("failed", "a")), 8000);
  let state = [];
  state = toastReducer(state, { type: "add", notice: notice("input", "a") });
  state = toastReducer(state, { type: "add", notice: notice("input", "b") });
  state = toastReducer(state, { type: "add", notice: notice("done", "a") });
  state = toastReducer(state, { type: "task", taskId: "a", status: "waiting" });
  assert.equal(state.length, 3);
  state = toastReducer(state, { type: "task", taskId: "a", status: "running" });
  assert.deepEqual(
    state.map((t) => [t.kind, t.taskId]),
    [
      ["input", "b"],
      ["done", "a"],
    ],
  );
});

test("the reveal store keeps a request for its repo's panels until it goes stale", () => {
  resetReveal();
  const seen = [];
  const off = subscribeReveal(() => seen.push("published"));
  const target = { taskId: "t1", repo: "/repo", focus: "question" };
  publishReveal(target, 1000);
  assert.deepEqual(seen, ["published"]);
  assert.equal(pendingReveal("/other", 1000), null);
  // A remounted panel still sees it, and gets the same object to dedupe on.
  assert.equal(pendingReveal("/repo", 1000), target);
  assert.equal(pendingReveal("/repo", 1000 + REVEAL_TTL_MS), target);
  assert.equal(pendingReveal("/repo", 1001 + REVEAL_TTL_MS), null);
  const next = { taskId: "t2", repo: "/repo", focus: "summary" };
  publishReveal(next, 5000);
  assert.equal(pendingReveal("/repo", 5000), next);
  off();
  publishReveal(target, 6000);
  assert.deepEqual(seen, ["published", "published"]);
  resetReveal();
});
