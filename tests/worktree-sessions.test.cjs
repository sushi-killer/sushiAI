const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  liveWorktreeBranches,
} = require("../src/workspace/worktreeSessions.ts");

const workspace = (name, panels, worktreeBranch) => ({
  id: name,
  name,
  cwd: "/w",
  panels,
  layout: null,
  ...(worktreeBranch ? { worktreeBranch } : {}),
});
const live = { id: "p1", kind: "agent", title: "Claude" };
const ended = { id: "p2", kind: "agent", title: "Claude", ended: true };

test("a workspace is a live worktree only through its branch and a live panel", () => {
  assert.deepEqual(
    liveWorktreeBranches([
      workspace("Client \u00b7 Q3", [live]),
      workspace("App \u00b7 plan-1", [ended], "plan-1"),
      workspace("App \u00b7 plan-2", [ended, live], "plan-2"),
      workspace("App", [live]),
    ]),
    ["plan-2"],
  );
});
