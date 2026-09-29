const { test } = require("node:test");
const assert = require("node:assert/strict");
const { orchestratorTarget } = require("../src/orchestrator/notices.ts");
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
