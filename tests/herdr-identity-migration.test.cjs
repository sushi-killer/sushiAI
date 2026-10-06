const { test } = require("node:test");
const assert = require("node:assert/strict");

const snapshot = import("../src/herdrSnapshot.ts");
test("different local sockets never share workspace or pane identities", async () => {
  const { reconcileHerdrWorkspaces } = await snapshot;
  const remote = {
    version: "1",
    workspaces: [{ workspace_id: "workspace-1", label: "App" }],
    panes: [
      { pane_id: "pane-1", workspace_id: "workspace-1", agent_status: "idle" },
    ],
  };
  const first = reconcileHerdrWorkspaces([], remote, "/tmp/first.sock");
  const both = reconcileHerdrWorkspaces(first, remote, "/tmp/second.sock");
  assert.equal(new Set(both.map((w) => w.id)).size, 2);
  assert.equal(new Set(both.flatMap((w) => w.panels.map((p) => p.id))).size, 2);
  const endedFirst = reconcileHerdrWorkspaces(
    both,
    { version: "1", workspaces: [], panes: [] },
    "/tmp/first.sock",
  );
  assert.equal(endedFirst[0].panels[0].ended, true);
  assert.equal(endedFirst[1].panels[0].ended, undefined);
});
