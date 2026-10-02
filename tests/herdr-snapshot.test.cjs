const { test } = require("node:test");
const assert = require("node:assert/strict");
const { herdrWorkspaceKey: key } = require("../src/workspace/worktree.ts");

test("Herdr reconciliation keeps the checkout and every saved workspace field", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const current = [
    {
      id: "saved-workspace",
      herdrId: "workspace-1",
      connection: "/tmp/herdr.sock",
      name: "App",
      cwd: "/work/checkout",
      panels: [],
      layout: null,
      metadata: { layoutOwner: "saved" },
    },
  ];
  const snapshot = {
    version: "1",
    workspaces: [
      {
        workspace_id: "workspace-1",
        label: "App",
        worktree: { checkout_path: "/work/checkout" },
      },
      { workspace_id: "intentional-copy", label: "Another session" },
    ],
    panes: [
      {
        pane_id: "a",
        workspace_id: "workspace-1",
        cwd: "/work/checkout/subdir",
        agent_status: "idle",
      },
      {
        pane_id: "b",
        workspace_id: "intentional-copy",
        cwd: "/work/checkout",
        agent_status: "idle",
      },
    ],
  };
  const next = reconcileHerdrWorkspaces(current, snapshot, "/tmp/herdr.sock");
  assert.equal(next[0].cwd, "/work/checkout");
  assert.deepEqual(next[0].metadata, { layoutOwner: "saved" });
  assert.equal(
    next.length,
    2,
    "intentional sessions in one checkout stay accessible",
  );
  const afterCd = reconcileHerdrWorkspaces(
    next,
    {
      ...snapshot,
      workspaces: snapshot.workspaces.map((w) => ({
        ...w,
        worktree: undefined,
      })),
      panes: snapshot.panes.map((p) => ({ ...p, cwd: "/work/elsewhere" })),
    },
    "/tmp/herdr.sock",
  );
  assert.equal(
    afterCd[0].cwd,
    "/work/checkout",
    "pane cd never changes a saved project's checkout",
  );
});

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
    [key("/tmp/herdr", "pane-1"), key("/tmp/herdr", "pane-2")],
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
    [key("/tmp/herdr", "pane-2"), key("/tmp/herdr", "pane-1"), "local"],
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
  assert.deepEqual(single[0].layout, {
    type: "leaf",
    id: key("/tmp/herdr", "only"),
  });

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
    id: key("/tmp/herdr", "only"),
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
      herdrTokens: {},
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
  assert.equal(
    contains(withNewPane[0].layout, key("/tmp/herdr", "pane-2")),
    true,
  );
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
      [key("local", "a"), undefined],
      [key("local", "b"), true],
    ],
  );
  assert.equal(ended[0].panels[1].status, undefined);
  assert.equal(ended[0].panels[1].kind, "agent");
  assert.strictEqual(
    ended[0].layout,
    first[0].layout,
    "the layout, and so the slot, is untouched",
  );
  assert.equal(
    JSON.stringify(ended[0].layout).includes(key("local", "b")),
    true,
  );
  // The same poll again changes nothing, down to object identity.
  assert.strictEqual(reconcileHerdrWorkspaces(ended, oneLeft, "local"), ended);
  // The pane coming back clears the mark and keeps its id.
  const back = reconcileHerdrWorkspaces(ended, both, "local");
  assert.equal(back[0].panels[1].id, key("local", "b"));
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
    { ...first[0], layout: { type: "leaf", id: key("local", "a") } },
  ];
  const next = reconcileHerdrWorkspaces(
    hidden,
    snap([{ workspace_id: "w1", label: "One" }], [pane("a")]),
    "local",
  );
  assert.deepEqual(
    next[0].panels.map((panel) => panel.id),
    [key("local", "a")],
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
      [pane("a", "w1", { cwd: "/one" }), pane("b", "w2", { cwd: "/two" })],
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
    [key("local", "w1"), key("local", "w2"), "plain"],
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
  assert.equal(next[0].id, key("local", "w1"));
  assert.deepEqual(
    next[0].panels.map((panel) => [panel.id, Boolean(panel.ended)]),
    [
      [key("local", "n"), false],
      [key("local", "a"), true],
    ],
  );
});

test("a dropped workspace is removed once the host has a live one at the same folder", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const host = "ssh:host-a";
  const first = reconcileHerdrWorkspaces(
    [],
    snap(
      [
        { workspace_id: "w5", label: "app" },
        { workspace_id: "w7", label: "docs" },
      ],
      [
        pane("p1", "w5", { cwd: "/srv/app" }),
        pane("p2", "w7", { cwd: "/srv/docs" }),
      ],
    ),
    host,
  );
  // The host restarted: w5 is gone and a session made w9 at the same folder.
  const next = reconcileHerdrWorkspaces(
    first,
    snap(
      [{ workspace_id: "w9", label: "app" }],
      [pane("p3", "w9", { cwd: "/srv/app" })],
    ),
    host,
  );
  assert.deepEqual(
    next.map((workspace) => workspace.id),
    [key(host, "w7"), key(host, "w9")],
  );
});

test("a new Herdr workspace keeps the project's id, order, name, and hidden chats", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const host = "ssh:host-a";
  const chat = { id: "chat", kind: "chat", title: "Transcript" };
  const other = {
    id: "other",
    name: "Other project",
    cwd: "/srv/other",
    panels: [],
    layout: null,
  };
  const empty = {
    id: key(host, "old-workspace"),
    herdrId: "old-workspace",
    connection: host,
    name: "Saved project",
    cwd: "/srv/app",
    panels: [chat],
    layout: null,
  };
  const next = reconcileHerdrWorkspaces(
    [other, empty],
    snap(
      [{ workspace_id: "new-workspace", label: "app" }],
      [pane("new-pane", "new-workspace", { cwd: "/srv/app" })],
    ),
    host,
  );
  assert.deepEqual(
    next.map((workspace) => workspace.id),
    [other.id, empty.id],
  );
  assert.equal(next[1].herdrId, "new-workspace");
  assert.equal(next[1].name, empty.name);
  assert.deepEqual(
    next[1].panels.map((panel) => panel.id),
    [key(host, "new-pane"), chat.id],
  );
  assert.deepEqual(next[1].layout, {
    type: "leaf",
    id: key(host, "new-pane"),
  });
});

test("a workspace keeps its folder when a pane cds away, and a remote one never gets the local home", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const first = reconcileHerdrWorkspaces(
    [],
    snap(
      [{ workspace_id: "w1", label: "App" }],
      [pane("p1", "w1", { cwd: "/srv/app" })],
    ),
    "ssh:host-a",
    "/local/home",
  );
  const moved = reconcileHerdrWorkspaces(
    first,
    snap(
      [{ workspace_id: "w1", label: "App" }],
      [pane("p1", "w1", { cwd: "/tmp" })],
    ),
    "ssh:host-a",
    "/local/home",
  );
  assert.equal(moved[0].cwd, "/srv/app");
  const unknown = reconcileHerdrWorkspaces(
    [],
    snap([{ workspace_id: "w2", label: "New" }], [pane("p2", "w2")]),
    "ssh:host-a",
    "/local/home",
  );
  assert.equal(unknown[0].cwd, "");
});

test("a dropped workspace folds into its folder's live one: its chat moves, its ended panes go", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const host = "ssh:host-a";
  const [first] = reconcileHerdrWorkspaces(
    [],
    snap(
      [{ workspace_id: "w1", label: "app" }],
      [pane("p1", "w1", { cwd: "/srv/app" })],
    ),
    host,
  );
  const withChat = {
    ...first,
    panels: [...first.panels, { id: "chat", kind: "chat", title: "Thread" }],
  };
  const next = reconcileHerdrWorkspaces(
    [withChat],
    snap(
      [{ workspace_id: "w2", label: "app" }],
      [pane("p2", "w2", { cwd: "/srv/app" })],
    ),
    host,
  );
  assert.deepEqual(
    next.map((workspace) => [
      workspace.id,
      workspace.panels.map((panel) => panel.id),
    ]),
    [[key(host, "w2"), [key(host, "p2"), "chat"]]],
  );
});

test("two dropped workspaces of one folder, nothing live, become one", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const workspaces = reconcileHerdrWorkspaces(
    [],
    snap(
      [
        { workspace_id: "w1", label: "app" },
        { workspace_id: "w2", label: "app" },
      ],
      [
        pane("p1", "w1", { cwd: "/srv/app" }),
        pane("p2", "w2", { cwd: "/srv/app" }),
      ],
    ),
    "local",
  );
  const withChat = workspaces.map((workspace, index) =>
    index === 1
      ? {
          ...workspace,
          panels: [
            ...workspace.panels,
            { id: "chat", kind: "chat", title: "Thread" },
          ],
        }
      : workspace,
  );
  // The host restarted and lists neither.
  const next = reconcileHerdrWorkspaces(withChat, snap([], []), "local");
  assert.deepEqual(
    next.map((workspace) => [
      workspace.id,
      workspace.panels.map((panel) => panel.id),
    ]),
    [[key("local", "w1"), [key("local", "p1"), "chat"]]],
  );
});

test("Herdr reconciliation keeps a pane's companion and records the pane's live folder", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const snapshot = (cwd) => ({
    version: "1",
    workspaces: [{ workspace_id: "ws-1", label: "Workspace" }],
    panes: [
      {
        pane_id: "pane-1",
        workspace_id: "ws-1",
        agent: "claude",
        agent_status: "idle",
        cwd,
      },
    ],
  });
  const first = reconcileHerdrWorkspaces(
    [],
    snapshot("/work/app"),
    "/tmp/herdr",
    "/home/test",
  );
  assert.equal(first[0].panels[0].paneCwd, "/work/app");
  const companion = {
    extensionId: "builtin.artifacts",
    surfaceId: "preview",
    args: { arg: "/work/app/plan.md" },
    open: true,
    ratio: 0.4,
  };
  const withCompanion = first.map((w) => ({
    ...w,
    panels: w.panels.map((p) => ({ ...p, companion })),
  }));
  const next = reconcileHerdrWorkspaces(
    withCompanion,
    snapshot("/work/app/sub"),
    "/tmp/herdr",
    "/home/test",
  );
  assert.deepEqual(next[0].panels[0].companion, companion);
  assert.equal(next[0].panels[0].paneCwd, "/work/app/sub");
  // The pane vanishing from the host keeps the slot, companion included.
  const gone = reconcileHerdrWorkspaces(
    next,
    { version: "1", workspaces: snapshot("/x").workspaces, panes: [] },
    "/tmp/herdr",
    "/home/test",
  );
  assert.deepEqual(gone[0].panels[0].companion, companion);
});

test("Herdr reconciliation keeps the worktree branch of a workspace", async () => {
  const { reconcileHerdrWorkspaces } = await import("../src/herdrSnapshot.ts");
  const current = [
    {
      id: "saved",
      herdrId: "ws-1",
      connection: "/tmp/herdr.sock",
      name: "App \u00b7 plan-1",
      cwd: "/work/app-plan-1",
      worktreeBranch: "plan-1",
      panels: [],
      layout: null,
    },
  ];
  const snapshot = {
    version: "1",
    workspaces: [{ workspace_id: "ws-1", label: "App \u00b7 plan-1" }],
    panes: [],
  };
  const next = reconcileHerdrWorkspaces(current, snapshot, "/tmp/herdr.sock");
  assert.equal(next[0].worktreeBranch, "plan-1");
  const gone = reconcileHerdrWorkspaces(
    next,
    { version: "1", workspaces: [], panes: [] },
    "/tmp/herdr.sock",
  );
  assert.equal(gone[0].worktreeBranch, "plan-1");
});
