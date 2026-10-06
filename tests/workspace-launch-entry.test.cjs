const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { createRequire } = require("node:module");
const ts = require("typescript");
const { SessionLauncher } = require("../electron/session-launch.cjs");
const { herdrWorkspaceKey } = require("../src/herdrIdentity.ts");
const { sessionPanelId } = require("../src/daemonSessions.ts");
const { applySessionLaunch } = require("../src/workspace/session-launch.ts");
const { leafIds } = require("../src/layout.ts");

function controller(
  bridge,
  initial,
  sourceFile = process.env.SUSHIAI_LAUNCH_BASELINE_SOURCE,
) {
  const entry = path.resolve(__dirname, "../src/workspace/useWorkspaces.ts");
  const source = fs.readFileSync(sourceFile || entry, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  const requireSource = createRequire(entry);
  const states = [];
  const react = {
    useCallback: (callback) => callback,
    useEffect: () => {},
    useRef: (current) => ({ current }),
    useState: (value) => {
      const index = states.length;
      states.push(value);
      return [
        value,
        (next) => {
          states[index] =
            typeof next === "function" ? next(states[index]) : next;
        },
      ];
    },
  };
  const exports = {};
  vm.runInNewContext(
    compiled,
    {
      exports,
      crypto,
      window: { bridge },
      require: (name) =>
        name === "react"
          ? react
          : name === "../TerminalPanel.tsx"
            ? { disposeTerminal: () => {} }
            : requireSource(name),
    },
    { filename: entry },
  );
  let workspaces = initial;
  const errors = [],
    invalidations = [],
    snapshots = [],
    confirmations = [];
  const ws = exports.useWorkspaces({
    workspaces,
    setWorkspaces: (update) => {
      workspaces = update(workspaces);
    },
    saved: null,
    socket: "/tmp/entry.sock",
    refreshHerdr: async (endpoint) => {
      snapshots.push(endpoint);
    },
    invalidateHerdr: (endpoint) => {
      invalidations.push(endpoint);
    },
    useEndpoint: () => {},
    notify: (message) => errors.push(message),
    showWorkspace: () => {},
    confirmClose: (value) => confirmations.push(value),
  });
  return {
    ws,
    current: () => workspaces,
    errors,
    invalidations,
    snapshots,
    confirmations,
    states,
    currentClosedProjects: () => states[5],
  };
}

function daemon(options = {}) {
  const workspaces = [],
    panes = [],
    calls = [];
  const connections = {
    socket: async (endpoint) => endpoint,
    inspect: async (_, input) => ({ cwd: input.root }),
  };
  const rpc = async (_, method, params) => {
    calls.push({ method, params });
    if (method === "session.snapshot")
      return { snapshot: { workspaces: [...workspaces], panes: [...panes] } };
    if (method === "workspace.create") {
      const workspace = { workspace_id: `w${workspaces.length + 1}` };
      const root_pane = {
        pane_id: `p${panes.length + 1}`,
        workspace_id: workspace.workspace_id,
        cwd: params.cwd,
      };
      workspaces.push(workspace);
      panes.push(root_pane);
      return { workspace, root_pane };
    }
    if (method === "pane.split") {
      const pane = {
        pane_id: `p${panes.length + 1}`,
        workspace_id: params.workspace_id,
        cwd: params.cwd,
      };
      panes.push(pane);
      return { pane };
    }
    if (method === "pane.send_input") return {};
    throw new Error(`Unexpected method ${method}`);
  };
  const launcher = new SessionLauncher({
    getConnections: () => connections,
    rpc,
    ...options,
  });
  return {
    bridge: {
      sessionLaunch: (input) => launcher.launch(input),
      herdr: rpc,
      terminalClose: async () => {},
    },
    workspaces,
    panes,
    calls,
  };
}

const initial = () => [
  {
    id: "local",
    name: "Local",
    cwd: "/tmp/checkout",
    panels: [],
    layout: null,
  },
];

test("new UI clicks and explicit intents do not consume a failed panel's retry", async () => {
  for (const operationId of [undefined, "new-request"]) {
    for (const action of ["create", "add"]) {
      const server = daemon();
      const first = controller(server.bridge, initial());
      await first.ws.createWorkspace(
        "Project",
        "/tmp/checkout",
        "herdr",
        "claude",
        "/tmp/entry.sock",
        "old-request",
      );
      const workspace = first.current().find((item) => item.connection);
      workspace.panels[0].launchError = "Preparation failed";
      workspace.panels[0].launchOperationId = "old-request";
      const app = controller(server.bridge, [workspace]);
      if (action === "create")
        await app.ws.createWorkspace(
          "Project",
          "/tmp/checkout",
          "herdr",
          "claude",
          "/tmp/entry.sock",
          operationId,
        );
      else
        await app.ws.addPanel(
          "agent",
          "claude",
          undefined,
          undefined,
          undefined,
          "herdr",
          workspace.id,
          undefined,
          operationId,
        );
      assert.equal(server.workspaces.length, 1);
      assert.equal(server.panes.length, 2, action);
      assert.equal(
        server.calls.filter((call) => call.method === "pane.split").length,
        1,
        action,
      );
      assert.equal(app.current()[0].panels[0].launchOperationId, "old-request");
      assert.equal(
        app.current()[0].panels[0].launchError,
        "Preparation failed",
      );
    }
  }
});

test("actual sushiAI project launch entry retains all 20 independent concurrent intentions", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  const results = await Promise.all(
    Array.from({ length: 20 }, () =>
      app.ws.createWorkspace("Checkout", "/tmp/checkout", "herdr", "shell"),
    ),
  );
  assert.ok(results.every(Boolean));
  assert.equal(server.workspaces.length, 1);
  assert.equal(server.panes.length, 20);
  assert.equal(
    app.current().filter((workspace) => workspace.connection).length,
    1,
  );
  assert.equal(
    app.current().find((workspace) => workspace.connection).panels.length,
    20,
  );
  assert.equal(app.invalidations.length, 40);
  assert.equal(app.snapshots.length, 20);
});

test("actual project entry request replay is idempotent while the next click creates a panel", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  await Promise.all([
    app.ws.createWorkspace(
      "Checkout",
      "/tmp/checkout",
      "herdr",
      "shell",
      "/tmp/entry.sock",
      "same-id",
    ),
    app.ws.createWorkspace(
      "Checkout",
      "/tmp/checkout",
      "herdr",
      "shell",
      "/tmp/entry.sock",
      "same-id",
    ),
  ]);
  assert.equal(server.workspaces.length, 1);
  assert.equal(server.panes.length, 1);
  await app.ws.createWorkspace("Checkout", "/tmp/checkout", "herdr", "shell");
  assert.equal(server.panes.length, 2);
});

test("actual add entry keeps 20 additions and explicit repeat opens one panel", async () => {
  const server = daemon();
  const result = await server.bridge.herdr(
    "/tmp/entry.sock",
    "workspace.create",
    { cwd: "/tmp/checkout" },
  );
  const workspace = {
    id: herdrWorkspaceKey("/tmp/entry.sock", result.workspace.workspace_id),
    connection: "/tmp/entry.sock",
    cwd: "/tmp/checkout",
    name: "Checkout",
    panels: [
      {
        id: herdrWorkspaceKey("/tmp/entry.sock", result.root_pane.pane_id),
        sessionId: result.root_pane.pane_id,
        kind: "terminal",
        title: "Shell",
      },
    ],
    layout: {
      type: "leaf",
      id: herdrWorkspaceKey("/tmp/entry.sock", result.root_pane.pane_id),
    },
  };
  const app = controller(server.bridge, [workspace]);
  await Promise.all(
    Array.from({ length: 20 }, () => app.ws.addPanel("terminal")),
  );
  assert.equal(server.panes.length, 21);
  await Promise.all([
    app.ws.addPanel(
      "terminal",
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      "same-add",
    ),
    app.ws.addPanel(
      "terminal",
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      "same-add",
    ),
  ]);
  assert.equal(server.panes.length, 22);
});

test("actual restore entry shares pending restoration and replaces the selected slot", async () => {
  const server = daemon();
  const panel = {
    id: "old",
    sessionId: "gone",
    kind: "terminal",
    title: "Shell",
    ended: true,
  };
  const workspace = {
    id: "workspace-old",
    connection: "/tmp/entry.sock",
    cwd: "/tmp/checkout",
    name: "Checkout",
    panels: [
      panel,
      {
        id: "chat",
        kind: "chat",
        title: "Thread",
        messages: [{ id: "m", role: "user", text: "Keep this conversation" }],
      },
    ],
    layout: {
      type: "split",
      id: "split",
      axis: "row",
      ratio: 0.35,
      a: { type: "leaf", id: "old" },
      b: { type: "leaf", id: "chat" },
    },
  };
  const original = server.bridge.herdr;
  server.bridge.herdr = (endpoint, method, params) =>
    method === "pane.split"
      ? Promise.reject(
          Object.assign(new Error("Missing"), { code: "workspace_not_found" }),
        )
      : original(endpoint, method, params);
  const launcher = new SessionLauncher({
    getConnections: () => ({
      socket: async (endpoint) => endpoint,
      inspect: async (_, input) => ({ cwd: input.root }),
    }),
    rpc: server.bridge.herdr,
  });
  server.bridge.sessionLaunch = (input) => launcher.launch(input);
  const app = controller(server.bridge, [workspace]);
  const first = app.ws.reopenPanel("old");
  const second = app.ws.reopenPanel("old");
  assert.equal(first, second);
  await first;
  assert.equal(server.workspaces.length, 1);
  assert.equal(server.panes.length, 1);
  const current = app.current()[0];
  assert.equal(current.layout.ratio, 0.35);
  assert.deepEqual(leafIds(current.layout), [
    sessionPanelId("/tmp/entry.sock", "p1"),
    "chat",
  ]);
  assert.equal(current.panels[1].messages[0].text, "Keep this conversation");
  assert.equal(app.states[1], sessionPanelId("/tmp/entry.sock", "p1"));
});

test("actual add and restore retain the upstream fifth account argument for Claude and Codex", async () => {
  const requests = [];
  const server = daemon({
    prepareSession: async (input) => {
      requests.push(input);
      return { prefix: "", settings: "", launch: "" };
    },
  });
  const opened = controller(server.bridge, initial());
  await opened.ws.createWorkspace(
    "Checkout",
    "/tmp/checkout",
    "herdr",
    "shell",
  );
  // The launch entry sends the app workspace id as the daemon group and the
  // launcher answers with it; session-launch.ts still matches the answer to
  // `herdrId`, so the fixture binds it that way until L4a rewrites both.
  const found = opened.current().find((item) => item.connection);
  const workspace = { ...found, herdrId: found.id };
  for (const [agent, accountId, field] of [
    ["claude", "claude-work", "claudeAccountId"],
    ["codex", "", "codexAccountId"],
  ]) {
    const app = controller(server.bridge, [workspace]);
    await app.ws.addPanel(
      "agent",
      agent,
      undefined,
      undefined,
      accountId,
      "herdr",
      workspace.id,
    );
    assert.equal(requests.at(-1)[field], accountId);
    const current = app.current()[0];
    const panel = current.panels.at(-1);
    assert.equal(panel[field], accountId);
    panel.ended = true;
    const restored = controller(server.bridge, [current]);
    await restored.ws.reopenPanel(panel.id);
    assert.equal(requests.at(-1)[field], accountId);
    assert.equal(restored.current()[0].panels.at(-1)[field], accountId);
  }
});

test("actual add forwards the selected worktree base and marks local worktree sessions", async () => {
  const workspace = {
    id: herdrWorkspaceKey("/tmp/entry.sock", "w1"),
    connection: "/tmp/entry.sock",
    name: "Checkout",
    cwd: "/tmp/checkout",
    panels: [{ id: "p1", sessionId: "p1", kind: "terminal", title: "Shell" }],
    layout: { type: "leaf", id: "p1" },
  };
  const chosen = {
    branch: "probe",
    base: "refs/remotes/origin/release/stable",
  };
  let request;
  const app = controller(
    {
      sessionLaunch: async (input) => {
        request = input;
        return {
          ok: true,
          value: {
            operationId: input.operationId,
            workspaceId: "worktree",
            paneId: "worktree-pane",
            cwd: "/tmp/checkout-probe",
            createdWorkspace: true,
          },
        };
      },
    },
    [workspace],
  );
  await app.ws.addPanel(
    "terminal",
    "claude",
    undefined,
    undefined,
    undefined,
    "herdr",
    workspace.id,
    chosen,
  );
  assert.deepEqual(request.worktree, chosen);
  let creation;
  const local = controller(
    {
      worktreeCreate: async (...args) => {
        creation = args;
        return { path: "/tmp/checkout-probe" };
      },
    },
    initial(),
  );
  await local.ws.addPanel(
    "terminal",
    "claude",
    undefined,
    undefined,
    undefined,
    "local",
    "local",
    chosen,
  );
  assert.deepEqual(Array.from(creation), [
    "/tmp/checkout",
    chosen.branch,
    chosen.base,
  ]);
  assert.equal(local.current().at(-1).localWorktree, true);
});

test("closing the last Herdr session keeps the project with no open panes", async () => {
  for (const chats of [false, true]) {
    const panel = {
      id: "live-pane",
      sessionId: "live-pane",
      kind: "terminal",
      title: "Shell",
    };
    const workspace = {
      id: herdrWorkspaceKey("/tmp/entry.sock", "w1"),
      connection: "/tmp/entry.sock",
      name: "Checkout",
      cwd: "/tmp/checkout",
      panels: [
        panel,
        ...(chats ? [{ id: "chat", kind: "chat", messages: [] }] : []),
      ],
      layout: { type: "leaf", id: panel.id },
    };
    const calls = [];
    const app = controller(
      {
        sessionClose: async (...args) => calls.push(args),
        terminalClose: async () => {},
      },
      [workspace],
    );
    app.ws.setProjectGit({ [workspace.id]: { linkedWorktree: false } });
    app.ws.closePanel(panel.id);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(app.confirmations.length, 0);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0], ["local", "live-pane", true]);
    const remaining = app.current().find((item) => item.id === workspace.id);
    assert.ok(remaining);
    assert.equal(remaining.cwd, workspace.cwd);
    assert.equal(remaining.name, workspace.name);
    assert.equal(remaining.layout, null);
    assert.equal(app.currentClosedProjects().length, 0);
    if (chats)
      assert.deepEqual(
        remaining.panels.map((item) => item.id),
        ["chat"],
      );
    else assert.deepEqual(remaining.panels, []);
  }
  const pending = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "pending-git"),
    connection: "/tmp/entry.sock",
    panels: [
      { id: "pending-pane", sessionId: "pending-pane", kind: "terminal" },
    ],
    layout: { type: "leaf", id: "pending-pane" },
  };
  const pendingApp = controller({ herdr: async () => {} }, [pending]);
  pendingApp.ws.closePanel("pending-pane");
  assert.equal(pendingApp.confirmations.length, 1);
  assert.equal(pendingApp.confirmations[0].workspace.id, pending.id);

  const linked = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "linked"),
    connection: "/tmp/entry.sock",
    panels: [{ id: "linked-pane", sessionId: "linked-pane", kind: "terminal" }],
    layout: { type: "leaf", id: "linked-pane" },
  };
  const linkedApp = controller({ herdr: async () => {} }, [linked]);
  linkedApp.ws.setProjectGit({
    [linked.id]: { linkedWorktree: true },
  });
  linkedApp.ws.closePanel("linked-pane");
  assert.equal(linkedApp.confirmations.length, 1);
  assert.equal(linkedApp.confirmations[0].workspace.id, linked.id);

  const local = {
    ...initial()[0],
    localWorktree: true,
    panels: [{ id: "local-shell", kind: "terminal", title: "Shell" }],
    layout: { type: "leaf", id: "local-shell" },
  };
  const closed = [];
  const app = controller({ terminalClose: async (id) => closed.push(id) }, [
    local,
  ]);
  app.ws.closePanel("local-shell");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(closed, []);
  assert.equal(app.confirmations.length, 1);
  assert.equal(app.confirmations[0].workspace.id, local.id);
  assert.equal(app.confirmations[0].panel.id, "local-shell");
  assert.equal(app.current()[0].id, local.id);
});

test("closed pane keeps a worktree workspace when checkout removal fails", async () => {
  const panel = { id: "worktree-pane", kind: "terminal", title: "Shell" };
  const workspace = {
    ...initial()[0],
    localWorktree: true,
    cwd: "/tmp/checkout",
    panels: [panel],
    layout: { type: "leaf", id: panel.id },
  };
  const app = controller(
    {
      terminalClose: async () => {},
      worktreeRemove: async () => {
        throw new Error("worktree is dirty");
      },
    },
    [workspace],
  );
  await app.ws.endSessions([{ workspace, panel }], {
    workspace,
    panel,
    checkout: workspace.cwd,
    branch: "feature/task",
  });
  assert.equal(app.current().length, 1);
  assert.equal(app.current()[0].id, workspace.id);
  assert.equal(app.current()[0].panels.length, 0);
  assert.match(app.errors.join(" "), /Worktree kept: worktree is dirty/);
});

test("closing a local worktree session without deleting its checkout keeps the project", async () => {
  const panel = { id: "worktree-pane", kind: "terminal", title: "Shell" };
  const workspace = {
    ...initial()[0],
    localWorktree: true,
    panels: [panel],
    layout: { type: "leaf", id: panel.id },
  };
  const app = controller({ terminalClose: async () => {} }, [workspace]);
  await app.ws.endSessions([{ workspace, panel }]);
  assert.equal(app.current().length, 1);
  assert.equal(app.current()[0].id, workspace.id);
  assert.deepEqual(app.current()[0].panels, []);
  assert.equal(app.current()[0].layout, null);
  assert.equal(app.currentClosedProjects().length, 0);
});

test("successful worktree cleanup leaves the project until Close Project", async () => {
  const panel = { id: "worktree-pane", kind: "terminal", title: "Shell" };
  const workspace = {
    ...initial()[0],
    localWorktree: true,
    cwd: "/tmp/checkout",
    panels: [panel],
    layout: { type: "leaf", id: panel.id },
  };
  const removals = [];
  const app = controller(
    {
      terminalClose: async () => {},
      worktreeRemove: async (...args) => removals.push(args),
    },
    [workspace],
  );
  await app.ws.endSessions([{ workspace, panel }], {
    workspace,
    panel,
    checkout: workspace.cwd,
    branch: "feature/task",
  });
  assert.equal(removals.length, 1);
  assert.equal(app.current().length, 1);
  assert.equal(app.current()[0].id, workspace.id);
  assert.deepEqual(app.current()[0].panels, []);
});

test("renaming an empty retained Herdr project does not call its vanished host workspace", async () => {
  const workspace = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "old-workspace"),
    connection: "/tmp/entry.sock",
    panels: [],
    layout: null,
  };
  const calls = [];
  const app = controller(
    {
      herdr: async (...args) => calls.push(args),
    },
    [workspace],
  );
  await app.ws.renameWorkspace(workspace.id, "Renamed project");
  assert.equal(app.current()[0].name, "Renamed project");
  assert.equal(calls.length, 0);
});

test("Close Project removes an empty retained Herdr project", async () => {
  const workspace = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "old-workspace"),
    connection: "/tmp/entry.sock",
    panels: [],
    layout: null,
  };
  const app = controller(
    {
      projectIdentify: async () => ({ projectId: "", remote: "" }),
    },
    [workspace],
  );
  await app.ws.endWorkspace(workspace);
  assert.equal(
    app.current().some((item) => item.id === workspace.id),
    false,
  );
  assert.equal(app.currentClosedProjects().length, 1);
  assert.equal(app.currentClosedProjects()[0].cwd, workspace.cwd);
});

test("closing a sole ended Herdr pane keeps the project empty", () => {
  const panel = {
    id: "ended-pane",
    sessionId: "ended-pane",
    kind: "terminal",
    title: "Shell",
    ended: true,
  };
  const workspace = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "w1"),
    connection: "/tmp/entry.sock",
    panels: [panel],
    layout: { type: "leaf", id: panel.id },
  };
  const app = controller({}, [workspace]);
  app.ws.closePanel(panel.id);
  const remaining = app.current();
  assert.equal(remaining.length, 1);
  assert.equal(remaining[0].id, workspace.id);
  assert.deepEqual(remaining[0].panels, []);
  assert.equal(remaining[0].layout, null);
});

test("adding a session to an empty retained Herdr project reuses its record", async () => {
  const server = daemon();
  const original = server.bridge.herdr;
  server.bridge.herdr = (endpoint, method, params) => {
    if (
      method === "pane.split" &&
      !server.workspaces.some(
        (workspace) => workspace.workspace_id === params.workspace_id,
      )
    )
      return Promise.reject(
        Object.assign(new Error("Missing"), { code: "workspace_not_found" }),
      );
    return original(endpoint, method, params);
  };
  server.bridge.sessionLaunch = (input) =>
    new SessionLauncher({
      getConnections: () => ({
        socket: async (endpoint) => endpoint,
        inspect: async (_, request) => ({ cwd: request.root }),
      }),
      rpc: server.bridge.herdr,
    }).launch(input);
  const workspace = {
    ...initial()[0],
    id: herdrWorkspaceKey("/tmp/entry.sock", "old-workspace"),
    connection: "/tmp/entry.sock",
    panels: [],
    layout: null,
  };
  const app = controller(server.bridge, [workspace]);
  await app.ws.addPanel("terminal");
  const remaining = app.current();
  assert.equal(remaining.length, 1);
  assert.equal(remaining[0].id, workspace.id);
  assert.equal(remaining[0].herdrId, "w1");
  assert.equal(remaining[0].panels.length, 1);
  assert.deepEqual(leafIds(remaining[0].layout), [remaining[0].panels[0].id]);
  assert.equal(app.states[0], workspace.id);
});

test("actual workspace close shares concurrent intent and allows retry after failure", async () => {
  const workspace = {
    ...initial()[0],
    connection: "/tmp/entry.sock",
    panels: [{ id: "p1", sessionId: "p1", kind: "terminal", title: "Shell" }],
    layout: { type: "leaf", id: "p1" },
  };
  let release;
  let closes = 0;
  let fail = true;
  const gate = new Promise((resolve) => (release = resolve));
  const app = controller(
    {
      sessionClose: async () => {
        closes += 1;
        await gate;
        if (fail) throw new Error("Close failed");
      },
      terminalClose: async () => {},
      projectIdentify: async () => ({ projectId: "", remote: "" }),
    },
    [workspace],
  );
  const first = app.ws.endWorkspace(workspace);
  await app.ws.endWorkspace(workspace);
  assert.equal(closes, 1);
  release();
  await first;
  assert.equal(app.current()[0].id, workspace.id);
  fail = false;
  await app.ws.endWorkspace(workspace);
  assert.equal(closes, 2);
  assert.equal(
    app.current().some((item) => item.id === workspace.id),
    false,
  );
  assert.equal(app.currentClosedProjects().length, 1);
  assert.equal(app.currentClosedProjects()[0].cwd, workspace.cwd);
});

test("launch result adoption preserves layout, local panels and separately listed sessions", () => {
  const owner = {
    id: "ended-workspace",
    name: "Name",
    connection: "/tmp/entry.sock",
    cwd: "/tmp/checkout",
    herdrId: "old-w",
    panels: [
      {
        id: "ended",
        herdrId: "ended",
        title: "Session",
        kind: "terminal",
        ended: true,
      },
      {
        id: "chat",
        kind: "chat",
        title: "Chat",
        messages: [{ id: "keep", role: "user", text: "keep" }],
      },
    ],
    layout: {
      type: "split",
      id: "s",
      axis: "row",
      ratio: 0.3,
      a: { type: "leaf", id: "ended" },
      b: { type: "leaf", id: "chat" },
    },
  };
  const value = {
    operationId: "restore",
    workspaceId: "new-w",
    paneId: "new-p",
    cwd: owner.cwd,
    createdWorkspace: true,
  };
  const panel = {
    id: herdrWorkspaceKey(owner.connection, "new-p"),
    herdrId: "new-p",
    title: "Session",
    kind: "terminal",
  };
  const listed = {
    ...owner,
    id: herdrWorkspaceKey(owner.connection, "new-w"),
    herdrId: "new-w",
    panels: [
      panel,
      {
        id: "other-live",
        herdrId: "other-live",
        kind: "terminal",
        title: "Other",
      },
    ],
    layout: null,
  };
  const unrelated = {
    ...owner,
    id: "separate",
    herdrId: "independent",
    panels: [
      { id: "living", herdrId: "living", kind: "terminal", title: "Keep" },
    ],
  };
  const output = applySessionLaunch(
    [owner, listed, unrelated],
    owner.connection,
    value,
    panel,
    { workspaceId: owner.id, panelId: "ended" },
  );
  assert.equal(output.length, 2);
  assert.equal(output[0].id, owner.id);
  assert.equal(output[0].panels.length, 3);
  assert.equal(output[0].panels[1].messages[0].text, "keep");
  assert.deepEqual(leafIds(output[0].layout), [panel.id, "chat", "other-live"]);
  assert.equal(output[1], unrelated);
});

test("renaming a session panel tells the daemon, a local panel renames in place", async () => {
  const calls = [];
  const workspace = {
    id: "w1",
    connection: "ssh:devbox",
    name: "Checkout",
    cwd: "/tmp/checkout",
    panels: [
      { id: "bound", sessionId: "s-1", kind: "terminal", title: "zsh" },
      { id: "plain", kind: "terminal", title: "zsh" },
    ],
    layout: { type: "leaf", id: "bound" },
  };
  const app = controller(
    {
      sessionUpdate: async (...args) => {
        calls.push(args);
      },
    },
    [workspace],
  );
  app.ws.renamePanel("bound", "Build");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(calls, [["devbox", { id: "s-1", title: "Build" }]]);
  assert.equal(app.current()[0].panels[0].title, "Build");
  app.ws.renamePanel("plain", "Notes");
  assert.equal(calls.length, 1);
  assert.equal(app.current()[0].panels[1].title, "Notes");
});

test("Reopen of an ended agent panel launches with resume and the workspace as group", async () => {
  const requests = [];
  const ended = {
    id: "old",
    sessionId: "s-gone",
    kind: "agent",
    agent: "claude",
    title: "Claude Code",
    ended: true,
    agentSession: "thread-3",
  };
  const workspace = {
    id: "w1",
    connection: "/tmp/entry.sock",
    name: "Checkout",
    cwd: "/tmp/checkout",
    panels: [ended],
    layout: { type: "leaf", id: "old" },
  };
  const app = controller(
    {
      sessionLaunch: async (input) => {
        requests.push(input);
        return {
          ok: true,
          value: {
            operationId: input.operationId,
            workspaceId: "w1",
            paneId: "s-new",
            cwd: "/tmp/checkout",
            createdWorkspace: false,
          },
        };
      },
      terminalClose: async () => {},
    },
    [workspace],
  );
  await app.ws.reopenPanel("old");
  assert.equal(requests.length, 1);
  assert.equal(requests[0].resume, "thread-3");
  assert.equal(requests[0].workspaceId, "w1");
  assert.equal(requests[0].agent, "claude");
  const [panel] = app.current()[0].panels;
  assert.equal(panel.sessionId, "s-new");
  assert.equal(panel.ended, undefined);
  assert.equal(panel.id, sessionPanelId("/tmp/entry.sock", "s-new"));
  assert.deepEqual(leafIds(app.current()[0].layout), [panel.id]);
});
