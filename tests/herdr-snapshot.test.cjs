const { test } = require("node:test");
const assert = require("node:assert/strict");

test("Herdr reconciliation indexes panes and keeps identical snapshots referentially stable", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const { tidy } = await import("../src/layout.ts");
  const snapshot = {
    version: "1",
    workspaces: [{ workspace_id: "ws-1", label: "Workspace" }],
    panes: [
      {
        pane_id: "pane-1",
        workspace_id: "ws-1",
        label: "Shell",
        agent_status: "idle",
      },
      {
        pane_id: "pane-2",
        workspace_id: "ws-1",
        agent: "codex",
        agent_status: "running",
      },
    ],
  };
  const first = reconcileHerdrWorkspaces(
    [],
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.equal(first.length, 1);
  assert.deepEqual(
    first[0].panels.map((panel) => panel.id),
    ["herdr:local:pane-1", "herdr:local:pane-2"],
  );
  assert.equal(first[0].panels[1].title, "Codex");

  const stable = reconcileHerdrWorkspaces(
    first,
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.strictEqual(stable, first);

  const partialRefresh = reconcileHerdrWorkspaces(
    first,
    {
      ...snapshot,
      panes: snapshot.panes.map((pane) =>
        pane.pane_id === "pane-1" ? { ...pane, agent_status: "blocked" } : pane,
      ),
    },
    "/tmp/herdr",
    "/home/test",
  );
  assert.notStrictEqual(partialRefresh[0].panels[0], first[0].panels[0]);
  assert.strictEqual(partialRefresh[0].panels[1], first[0].panels[1]);

  const withMetadata = [
    {
      ...first[0],
      panels: first[0].panels.map((panel) => ({
        ...panel,
        note: "changed",
        status: "stale",
      })),
    },
  ];
  const metadataRefresh = reconcileHerdrWorkspaces(
    withMetadata,
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.notStrictEqual(metadataRefresh, withMetadata);
  assert.equal(metadataRefresh[0].panels[0].note, "changed");

  const withLocalPanel = [
    {
      ...first[0],
      panels: [
        ...first[0].panels,
        { id: "local", kind: "files", title: "Notes" },
      ],
      layout: tidy([...first[0].panels.map((panel) => panel.id), "local"]),
    },
  ];
  const changed = reconcileHerdrWorkspaces(
    withLocalPanel,
    {
      ...snapshot,
      panes: [snapshot.panes[1]],
    },
    "/tmp/herdr",
    "/home/test",
  );
  assert.deepEqual(
    changed[0].panels.map((panel) => panel.id),
    ["herdr:local:pane-2", "local"],
  );
  assert.equal(changed[0].connection, "/tmp/herdr");

  const otherEndpoint = reconcileHerdrWorkspaces(
    [...withLocalPanel, { ...first[0], id: "remote", connection: "ssh:other" }],
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.equal(otherEndpoint.length, 2);
  assert.equal(otherEndpoint[0].connection, "ssh:other");
});

test("Herdr reconciliation builds a large workspace without recursive layout overflow", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const panes = Array.from({ length: 12000 }, (_, index) => ({
    pane_id: `pane-${index}`,
    workspace_id: "large",
    agent_status: "idle",
  }));
  const result = reconcileHerdrWorkspaces(
    [],
    {
      version: "1",
      workspaces: [{ workspace_id: "large", label: "Large" }],
      panes,
    },
    "/tmp/herdr",
  );
  assert.equal(result[0].panels.length, panes.length);
  assert.equal(result[0].layout.type, "split");

  const reduced = reconcileHerdrWorkspaces(
    result,
    {
      version: "1",
      workspaces: [{ workspace_id: "large", label: "Large" }],
      panes: panes.slice(0, -1),
    },
    "/tmp/herdr",
  );
  assert.equal(reduced[0].panels.length, panes.length - 1);

  const single = reconcileHerdrWorkspaces(
    [],
    {
      version: "1",
      workspaces: [{ workspace_id: "single", label: "Single" }],
      panes: [
        { pane_id: "only", workspace_id: "single", agent_status: "idle" },
      ],
    },
    "/tmp/herdr",
  );
  assert.deepEqual(single[0].layout, { type: "leaf", id: "herdr:local:only" });

  const importedIntoEmpty = reconcileHerdrWorkspaces(
    [
      {
        id: "existing",
        name: "Single",
        cwd: "/tmp",
        connection: "/tmp/herdr",
        herdrId: "single",
        panels: [],
        layout: null,
      },
    ],
    {
      version: "1",
      workspaces: [{ workspace_id: "single", label: "Single" }],
      panes: [
        { pane_id: "only", workspace_id: "single", agent_status: "idle" },
      ],
    },
    "/tmp/herdr",
  );
  assert.deepEqual(importedIntoEmpty[0].layout, {
    type: "leaf",
    id: "herdr:local:only",
  });
});

test("Herdr refresh keeps hidden sessions and chat-only panels out of Code", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const { contains } = await import("../src/layout.ts");
  const current = [
    {
      id: "workspace",
      name: "Workspace",
      cwd: "/home/test",
      connection: "/tmp/herdr",
      herdrId: "ws-1",
      panels: [
        {
          id: "herdr:local:pane-1",
          kind: "terminal",
          title: "Shell",
          herdrId: "pane-1",
          agent: undefined,
          status: "idle",
        },
        { id: "chat-only", kind: "chat", title: "Thread", messages: [] },
      ],
      layout: null,
    },
  ];
  const snapshot = {
    version: "1",
    workspaces: [{ workspace_id: "ws-1", label: "Workspace" }],
    panes: [
      {
        pane_id: "pane-1",
        workspace_id: "ws-1",
        label: "Shell",
        agent_status: "idle",
      },
    ],
  };

  const stable = reconcileHerdrWorkspaces(
    current,
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.strictEqual(stable, current);

  const withNewPane = reconcileHerdrWorkspaces(
    current,
    {
      ...snapshot,
      panes: [
        ...snapshot.panes,
        {
          pane_id: "pane-2",
          workspace_id: "ws-1",
          label: "New shell",
          agent_status: "idle",
        },
      ],
    },
    "/tmp/herdr",
    "/home/test",
  );
  assert.equal(contains(withNewPane[0].layout, "herdr:local:pane-2"), true);
  assert.equal(contains(withNewPane[0].layout, "herdr:local:pane-1"), false);
  assert.equal(contains(withNewPane[0].layout, "chat-only"), false);
});
