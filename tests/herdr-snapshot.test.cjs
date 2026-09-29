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
        agent_status: "working",
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
    ["herdr:local:pane-2", "herdr:local:pane-1", "local"],
  );
  assert.equal(changed[0].panels[1].ended, true);
  assert.equal(changed[0].panels[0].ended, undefined);
  assert.equal(changed[0].connection, "/tmp/herdr");

  // A poll for "/tmp/herdr" must only ever update that connection's own
  // workspace in place, never reorder one belonging to a different
  // connection - otherwise every independent poll (the app polls every
  // connected host concurrently) would reshuffle the whole array.
  const otherEndpointSource = [
    ...withLocalPanel,
    { ...first[0], id: "remote", connection: "ssh:other" },
  ];
  const otherEndpoint = reconcileHerdrWorkspaces(
    otherEndpointSource,
    snapshot,
    "/tmp/herdr",
    "/home/test",
  );
  assert.equal(otherEndpoint.length, 2);
  assert.equal(otherEndpoint[0].connection, "/tmp/herdr");
  assert.strictEqual(otherEndpoint[1], otherEndpointSource[1]);
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
  assert.equal(reduced[0].panels.length, panes.length);
  assert.equal(reduced[0].panels.filter((panel) => panel.ended).length, 1);

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

const snap = (workspaces, panes) => ({ version: "1", workspaces, panes });
const pane = (id, workspace = "w1", extra = {}) => ({
  pane_id: id,
  workspace_id: workspace,
  agent_status: "idle",
  ...extra,
});

test("a pane missing from the snapshot keeps its layout slot, marked ended", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const both = snap(
    [{ workspace_id: "w1", label: "One" }],
    [pane("a"), pane("b", "w1", { agent: "claude", agent_status: "working" })],
  );
  const first = reconcileHerdrWorkspaces([], both, "local");
  const oneLeft = snap([{ workspace_id: "w1", label: "One" }], [pane("a")]);
  const ended = reconcileHerdrWorkspaces(first, oneLeft, "local");
  assert.deepEqual(
    ended[0].panels.map((panel) => [panel.id, panel.ended]),
    [
      ["herdr:local:a", undefined],
      ["herdr:local:b", true],
    ],
  );
  assert.equal(ended[0].panels[1].status, undefined);
  assert.equal(ended[0].panels[1].kind, "agent");
  assert.strictEqual(
    ended[0].layout,
    first[0].layout,
    "the layout, and so the slot, is untouched",
  );
  assert.equal(JSON.stringify(ended[0].layout).includes("herdr:local:b"), true);
  // The same poll again changes nothing, down to object identity.
  assert.strictEqual(reconcileHerdrWorkspaces(ended, oneLeft, "local"), ended);
  // The pane coming back clears the mark and keeps its id.
  const back = reconcileHerdrWorkspaces(ended, both, "local");
  assert.equal(back[0].panels[1].id, "herdr:local:b");
  assert.equal("ended" in back[0].panels[1], false);
});

test("a pane hidden from the layout is dropped, not ended, when it goes", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const first = reconcileHerdrWorkspaces(
    [],
    snap([{ workspace_id: "w1", label: "One" }], [pane("a"), pane("b")]),
    "local",
  );
  const hidden = [
    { ...first[0], layout: { type: "leaf", id: "herdr:local:a" } },
  ];
  const next = reconcileHerdrWorkspaces(
    hidden,
    snap([{ workspace_id: "w1", label: "One" }], [pane("a")]),
    "local",
  );
  assert.deepEqual(
    next[0].panels.map((panel) => panel.id),
    ["herdr:local:a"],
  );
});

test("a workspace missing from the snapshot stays with every Herdr pane ended", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const first = reconcileHerdrWorkspaces(
    [],
    snap(
      [
        { workspace_id: "w1", label: "One" },
        { workspace_id: "w2", label: "Two" },
      ],
      [pane("a"), pane("b", "w2")],
    ),
    "local",
  );
  const withLocal = [
    ...first,
    {
      id: "plain",
      name: "Plain",
      cwd: "/tmp",
      panels: [{ id: "t", kind: "terminal", title: "zsh" }],
      layout: { type: "leaf", id: "t" },
    },
  ];
  const next = reconcileHerdrWorkspaces(
    withLocal,
    snap([{ workspace_id: "w2", label: "Two" }], [pane("b", "w2")]),
    "local",
  );
  assert.deepEqual(
    next.map((workspace) => workspace.id),
    ["herdr:local:w1", "herdr:local:w2", "plain"],
  );
  assert.equal(next[0].panels[0].ended, true);
  assert.strictEqual(next[0].layout, first[0].layout);
  assert.deepEqual(next[1], first[1]);
  assert.strictEqual(next[2], withLocal[2]);
  const again = reconcileHerdrWorkspaces(
    next,
    snap([{ workspace_id: "w2", label: "Two" }], [pane("b", "w2")]),
    "local",
  );
  assert.strictEqual(again, next);
});

test("a workspace rebound to a new Herdr id adopts it and keeps its panels", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const first = reconcileHerdrWorkspaces(
    [],
    snap([{ workspace_id: "w1", label: "One" }], [pane("a")]),
    "local",
  );
  const ended = reconcileHerdrWorkspaces(first, snap([], []), "local");
  assert.equal(ended[0].panels[0].ended, true);
  const rebound = [{ ...ended[0], herdrId: "w9" }];
  const next = reconcileHerdrWorkspaces(
    rebound,
    snap([{ workspace_id: "w9", label: "One" }], [pane("n", "w9")]),
    "local",
  );
  assert.equal(next.length, 1);
  assert.equal(next[0].id, "herdr:local:w1");
  assert.deepEqual(
    next[0].panels.map((panel) => [panel.id, Boolean(panel.ended)]),
    [
      ["herdr:local:n", false],
      ["herdr:local:a", true],
    ],
  );
});
