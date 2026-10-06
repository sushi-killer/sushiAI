const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { createRequire } = require("node:module");
const ts = require("typescript");
const { herdrWorkspaceKey } = require("../src/herdrIdentity.ts");
const { sessionPanelId } = require("../src/daemonSessions.ts");
const {
  launchTarget,
  placeLaunchedPanel,
} = require("../src/workspace/session-launch.ts");
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

// A fake daemon bridge: one session per idempotency key, like the real one.
function daemon({ fail } = {}) {
  const sessions = new Map();
  const requests = [];
  const bridge = {
    daemonSessionLaunch: async (request) => {
      requests.push(request);
      if (fail?.(request)) throw new Error("Launch failed");
      if (!sessions.has(request.idempotencyKey))
        sessions.set(request.idempotencyKey, `s${sessions.size + 1}`);
      return {
        host: request.host,
        sessionId: sessions.get(request.idempotencyKey),
        cwd: request.worktree
          ? `${request.cwd}-${request.worktree.branch}`
          : request.cwd,
      };
    },
    terminalClose: async () => {},
  };
  return { bridge, requests, sessions };
}

const entry = "/tmp/entry.sock";
const hostWorkspace = (panels = [], layout = null) => ({
  id: "w1",
  connection: entry,
  cwd: "/tmp/checkout",
  name: "Checkout",
  panels,
  layout,
});

const initial = () => [
  {
    id: "local",
    name: "Local",
    cwd: "/tmp/checkout",
    panels: [],
    layout: null,
  },
];

test("a failed launch keeps its operation id for the retry and leaves no panel", async () => {
  let failing = true;
  const server = daemon({ fail: () => failing });
  const app = controller(server.bridge, initial());
  const create = (operationId) =>
    app.ws.createWorkspace(
      "Project",
      "/tmp/checkout",
      "claude",
      entry,
      operationId,
    );
  assert.equal(await create("old-request"), false);
  assert.equal(app.errors.length, 1);
  assert.equal(app.current().filter((item) => item.connection).length, 0);
  failing = false;
  assert.equal(await create("old-request"), true);
  assert.equal(server.sessions.size, 1);
  assert.deepEqual(
    server.requests.map((request) => request.idempotencyKey),
    ["old-request", "old-request"],
  );
  await create(undefined);
  assert.equal(server.sessions.size, 2);
  assert.notEqual(server.requests[2].idempotencyKey, "old-request");
});

test("20 concurrent new-project launches each get their own session and workspace", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  const results = await Promise.all(
    Array.from({ length: 20 }, () =>
      app.ws.createWorkspace("Checkout", "/tmp/checkout", "shell"),
    ),
  );
  assert.ok(results.every(Boolean));
  assert.equal(server.sessions.size, 20);
  const hosted = app.current().filter((workspace) => workspace.connection);
  assert.equal(hosted.length, 20);
  assert.ok(hosted.every((workspace) => workspace.panels.length === 1));
  assert.equal(app.snapshots.length, 20);
  assert.equal(server.requests[0].host, "local");
});

test("a replayed project launch is idempotent while the next click creates a session", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  await Promise.all(
    [0, 1].map(() =>
      app.ws.createWorkspace(
        "Checkout",
        "/tmp/checkout",
        "shell",
        entry,
        "same-id",
      ),
    ),
  );
  assert.equal(server.sessions.size, 1);
  const hosted = app.current().filter((workspace) => workspace.connection);
  assert.equal(hosted.length, 1);
  assert.equal(hosted[0].panels.length, 1);
  assert.equal(hosted[0].panels[0].id, sessionPanelId(entry, "s1"));
  await app.ws.createWorkspace("Checkout", "/tmp/checkout", "shell");
  assert.equal(server.sessions.size, 2);
});

test("20 additions land in one workspace and an explicit replay opens one panel", async () => {
  const server = daemon();
  const app = controller(server.bridge, [hostWorkspace()]);
  await Promise.all(
    Array.from({ length: 20 }, () => app.ws.addPanel("terminal")),
  );
  assert.equal(server.sessions.size, 20);
  assert.equal(app.current()[0].panels.length, 20);
  const replay = () =>
    app.ws.addPanel(
      "terminal",
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
      "same-add",
    );
  await Promise.all([replay(), replay()]);
  assert.equal(server.sessions.size, 21);
  assert.equal(app.current()[0].panels.length, 21);
});

test("Reopen shares one pending launch and replaces the selected slot", async () => {
  const server = daemon();
  const panel = {
    id: "old",
    sessionId: "gone",
    kind: "terminal",
    title: "Shell",
    ended: true,
  };
  const workspace = {
    ...hostWorkspace(
      [
        panel,
        {
          id: "chat",
          kind: "chat",
          title: "Thread",
          messages: [{ id: "m", role: "user", text: "Keep this conversation" }],
        },
      ],
      {
        type: "split",
        id: "split",
        axis: "row",
        ratio: 0.35,
        a: { type: "leaf", id: "old" },
        b: { type: "leaf", id: "chat" },
      },
    ),
    id: "workspace-old",
  };
  const app = controller(server.bridge, [workspace]);
  const first = app.ws.reopenPanel("old");
  const second = app.ws.reopenPanel("old");
  assert.equal(first, second);
  await first;
  assert.equal(server.sessions.size, 1);
  const current = app.current()[0];
  assert.equal(current.layout.ratio, 0.35);
  assert.deepEqual(leafIds(current.layout), [
    sessionPanelId(entry, "s1"),
    "chat",
  ]);
  assert.equal(current.panels[1].messages[0].text, "Keep this conversation");
  assert.equal(app.states[1], sessionPanelId(entry, "s1"));
});

test("add and Reopen carry the Claude and Codex account to the daemon launch", async () => {
  const server = daemon();
  for (const [agent, accountId, field] of [
    ["claude", "claude-work", "claudeAccountId"],
    ["codex", "", "codexAccountId"],
  ]) {
    const app = controller(server.bridge, [hostWorkspace()]);
    await app.ws.addPanel(
      "agent",
      agent,
      undefined,
      undefined,
      accountId,
      "w1",
    );
    assert.equal(server.requests.at(-1)[field], accountId);
    const current = app.current()[0];
    const panel = current.panels.at(-1);
    assert.equal(panel[field], accountId);
    panel.ended = true;
    const restored = controller(server.bridge, [current]);
    await restored.ws.reopenPanel(panel.id);
    assert.equal(server.requests.at(-1)[field], accountId);
    assert.equal(restored.current()[0].panels.at(-1)[field], accountId);
  }
});

test("add forwards the selected worktree base into a workspace of its own", async () => {
  const server = daemon();
  const workspace = hostWorkspace(
    [{ id: "p1", sessionId: "p1", kind: "terminal", title: "Shell" }],
    { type: "leaf", id: "p1" },
  );
  const chosen = {
    branch: "probe",
    base: "refs/remotes/origin/release/stable",
  };
  const app = controller(server.bridge, [workspace]);
  await app.ws.addPanel(
    "terminal",
    "claude",
    undefined,
    undefined,
    undefined,
    workspace.id,
    chosen,
  );
  assert.deepEqual(server.requests[0].worktree, chosen);
  const [original, created] = app.current();
  assert.equal(original.panels.length, 1);
  assert.equal(created.cwd, "/tmp/checkout-probe");
  assert.equal(created.worktreeBranch, "probe");
  assert.equal(created.connection, entry);
  assert.equal(created.panels[0].sessionId, "s1");
  assert.equal(created.id, server.requests[0].group);
  // A workspace without a connection launches its worktree in the local daemon.
  const local = controller(server.bridge, initial());
  await local.ws.addPanel(
    "terminal",
    "claude",
    undefined,
    undefined,
    undefined,
    "local",
    chosen,
  );
  const request = server.requests.at(-1);
  assert.equal(request.host, "local");
  assert.deepEqual(request.worktree, chosen);
  const made = local.current().at(-1);
  assert.equal(made.id, request.group);
  assert.notEqual(made.id, "local");
  assert.equal(made.panels[0].sessionId.startsWith("s"), true);
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
    // A live session is never ended without the dialog.
    assert.equal(app.confirmations.length, 1);
    assert.equal(calls.length, 0);
    await app.ws.endSessions([{ workspace, panel }]);
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

test("adding a session to an empty retained project reuses its record", async () => {
  const server = daemon();
  const workspace = {
    ...initial()[0],
    id: herdrWorkspaceKey(entry, "old-workspace"),
    connection: entry,
    panels: [],
    layout: null,
  };
  const app = controller(server.bridge, [workspace]);
  await app.ws.addPanel("terminal");
  const remaining = app.current();
  assert.equal(remaining.length, 1);
  assert.equal(remaining[0].id, workspace.id);
  assert.equal(remaining[0].panels.length, 1);
  assert.equal(remaining[0].panels[0].id, sessionPanelId(entry, "s1"));
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

test("a launched panel takes the ended slot, joins its workspace or opens a new one", () => {
  const owner = {
    id: "ended-workspace",
    name: "Name",
    connection: entry,
    cwd: "/tmp/checkout",
    panels: [
      {
        id: "ended",
        sessionId: "gone",
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
  const unrelated = { ...owner, id: "separate", panels: [], layout: null };
  const panel = {
    id: sessionPanelId(entry, "new"),
    sessionId: "new",
    title: "Session",
    kind: "terminal",
  };
  const request = { endpoint: entry, label: "Name", cwd: owner.cwd };
  const all = [owner, unrelated];
  const restore = { workspaceId: owner.id, panelId: "ended" };
  assert.equal(launchTarget(all, request, panel.id, restore), owner.id);
  const reopened = placeLaunchedPanel(all, {
    targetId: owner.id,
    request,
    panel,
    cwd: owner.cwd,
    restore,
  });
  assert.equal(reopened.length, 2);
  assert.equal(reopened[0].panels[1].messages[0].text, "keep");
  assert.deepEqual(leafIds(reopened[0].layout), [panel.id, "chat"]);
  assert.equal(reopened[1], unrelated);
  // A new panel joins the workspace that asked; a replay finds it again.
  const asked = { ...request, workspaceId: "separate" };
  assert.equal(launchTarget(all, asked, panel.id), "separate");
  const joined = placeLaunchedPanel(all, {
    targetId: "separate",
    request: asked,
    panel,
    cwd: owner.cwd,
  });
  assert.deepEqual(leafIds(joined[1].layout), [panel.id]);
  assert.equal(launchTarget(joined, asked, panel.id), "separate");
  assert.equal(
    placeLaunchedPanel(joined, {
      targetId: "other",
      request: asked,
      panel,
      cwd: owner.cwd,
    }).length,
    2,
  );
  // A worktree launch never joins the asking workspace.
  const worktree = { ...asked, worktree: { branch: "probe" } };
  assert.equal(launchTarget(all, worktree, panel.id), undefined);
  const created = placeLaunchedPanel(all, {
    targetId: "fresh",
    request: worktree,
    panel,
    cwd: "/tmp/checkout-probe",
  });
  assert.equal(created.length, 3);
  assert.deepEqual(
    { ...created[2], panels: undefined, layout: undefined },
    {
      id: "fresh",
      name: "Name",
      cwd: "/tmp/checkout-probe",
      connection: entry,
      worktreeBranch: "probe",
      panels: undefined,
      layout: undefined,
    },
  );
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
  const workspace = hostWorkspace([ended], { type: "leaf", id: "old" });
  const app = controller(
    {
      daemonSessionLaunch: async (input) => {
        requests.push(input);
        return { host: "local", sessionId: "s-new", cwd: "/tmp/checkout" };
      },
      terminalClose: async () => {},
    },
    [workspace],
  );
  await app.ws.reopenPanel("old");
  assert.equal(requests.length, 1);
  assert.equal(requests[0].resume, "thread-3");
  assert.equal(requests[0].group, "w1");
  assert.equal(requests[0].agent, "claude");
  assert.equal(requests[0].host, "local");
  assert.ok(requests[0].idempotencyKey);
  const [panel] = app.current()[0].panels;
  assert.equal(panel.sessionId, "s-new");
  assert.equal(panel.ended, undefined);
  assert.equal(panel.id, sessionPanelId(entry, "s-new"));
  assert.deepEqual(leafIds(app.current()[0].layout), [panel.id]);
});

test("the default workspace launches every panel kind through the local daemon", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  for (const [kind, agent] of [
    ["terminal", "claude"],
    ["agent", "claude"],
    ["agent", "codex"],
    ["agent", "gemini"],
  ])
    assert.equal(
      await app.ws.addPanel(
        kind,
        agent,
        undefined,
        undefined,
        undefined,
        "local",
      ),
      true,
    );
  assert.equal(server.requests.length, 4);
  assert.deepEqual(
    server.requests.map((r) => [r.host, r.agent]),
    [
      ["local", undefined],
      ["local", "claude"],
      ["local", "codex"],
      ["local", "gemini"],
    ],
  );
  const panels = app.current()[0].panels;
  assert.equal(panels.length, 4);
  assert.ok(panels.every((p) => p.sessionId));
  assert.deepEqual(
    panels.map((p) => p.agent),
    [undefined, "claude", "codex", "gemini"],
  );
});

test("a panel that never ran starts through the daemon in its own slot", async () => {
  const server = daemon();
  const starter = {
    id: "starter",
    kind: "agent",
    agent: "codex",
    title: "Codex",
  };
  const app = controller(server.bridge, [
    {
      ...initial()[0],
      panels: [starter],
      layout: { type: "leaf", id: "starter" },
    },
  ]);
  app.ws.startPanel("starter");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(server.requests.length, 1);
  assert.equal(server.requests[0].agent, "codex");
  const [panel] = app.current()[0].panels;
  assert.equal(panel.agent, "codex");
  assert.ok(panel.sessionId);
});

test("createWorkspace sends the new workspace id as the daemon group", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  await app.ws.createWorkspace("Fresh", "/tmp/fresh", "codex", entry, "op-1");
  assert.equal(server.requests[0].group, app.current()[1].id);
  assert.equal(server.requests[0].agent, "codex");
});

test("reopenProject launches a shell on the remembered endpoint and forgets the project", async () => {
  const server = daemon();
  const app = controller(server.bridge, initial());
  await app.ws.reopenProject({
    id: "p",
    name: "Old",
    cwd: "/tmp/old",
    endpoint: entry,
    backed: true,
    closedAt: 1,
    git: {},
  });
  assert.deepEqual(app.errors, []);
  assert.equal(server.requests.length, 1);
  assert.equal(server.requests[0].cwd, "/tmp/old");
  assert.equal(server.requests[0].agent, undefined);
  assert.equal(app.current().at(-1).cwd, "/tmp/old");
});

test("an agent panel keeps its agent when a terminal event names none", () => {
  const { retitleTerminal } = require("../src/workspace/workspace-actions.ts");
  const panel = { id: "p", kind: "agent", agent: "codex", title: "Codex" };
  assert.equal(retitleTerminal(panel, undefined).agent, "codex");
  assert.equal(retitleTerminal(panel, null).agent, "codex");
});

test("closing any live daemon session asks first, an ended one closes without asking", () => {
  for (const [kind, agent] of [
    ["terminal", undefined],
    ["agent", "claude"],
    ["agent", "codex"],
    ["agent", "gemini"],
    ["agent", "cursor-agent"],
  ]) {
    const live = { id: "live", sessionId: "s1", kind, agent, title: "x" };
    const app = controller({ terminalClose: async () => {} }, [
      { ...initial()[0], panels: [live], layout: { type: "leaf", id: "live" } },
    ]);
    app.ws.closePanel("live");
    assert.equal(app.confirmations.length, 1, `${kind} ${agent}`);
    assert.equal(app.current()[0].panels.length, 1);
  }
  const ended = {
    id: "e",
    sessionId: "s2",
    kind: "agent",
    title: "x",
    ended: true,
  };
  const app = controller({ terminalClose: async () => {} }, [
    { ...initial()[0], panels: [ended], layout: { type: "leaf", id: "e" } },
  ]);
  app.ws.closePanel("e");
  assert.equal(app.confirmations.length, 0);
});

test("a terminal event never changes the agent of a daemon session panel", () => {
  const { retitleTerminal } = require("../src/workspace/workspace-actions.ts");
  const panel = {
    id: "p",
    kind: "agent",
    agent: "codex",
    sessionId: "s1",
    title: "Codex",
  };
  assert.equal(retitleTerminal(panel, "claude"), panel);
});
