const { test } = require("node:test");
const assert = require("node:assert/strict");
const library = import("../src/workspaceState.ts");

const panel = (id, kind = "terminal", extra = {}) => ({
  id,
  kind,
  title: id,
  ...extra,
});

test("restore normalizes durable state and resets transient panel runtime", async () => {
  const { restore } = await library;
  const remote = {
    id: "remote",
    name: "Remote",
    cwd: "/remote",
    herdrId: "herdr-1",
    panels: [
      panel("terminal", "terminal", {
        busy: true,
        started: true,
        status: "working",
        note: "keep metadata",
        messages: [
          { id: "u", role: "user", text: "" },
          { id: "a-empty", role: "assistant", text: "" },
          { id: "a", role: "assistant", text: "done" },
        ],
      }),
    ],
    layout: { type: "leaf", id: "terminal" },
  };
  const data = JSON.stringify({
    workspaces: [remote],
    socket: "ssh:origin",
    mode: "invalid",
    tabMode: 1,
    section: 42,
    selected: null,
    zoomed: "",
    sidebar: "yes",
  });
  const saved = restore({ read: () => data });
  assert.equal(saved.mode, "Code");
  assert.equal(saved.tabMode, false);
  assert.equal(saved.section, "");
  assert.equal(saved.selected, "");
  assert.equal(saved.zoomed, "");
  assert.equal(saved.sidebar, undefined);
  assert.equal(saved.workspaces[0].connection, "ssh:origin");
  assert.equal(saved.workspaces[0].panels[0].busy, false);
  assert.equal(saved.workspaces[0].panels[0].started, false);
  assert.equal(saved.workspaces[0].panels[0].status, "working");
  assert.deepEqual(
    saved.workspaces[0].panels[0].messages.map((message) => message.id),
    ["u", "a"],
  );
});

test("restore preserves explicit Herdr connection and clears local connection", async () => {
  const { restore } = await library;
  const workspaces = [
    {
      id: "explicit",
      name: "Explicit",
      cwd: "/a",
      herdrId: "one",
      connection: "ssh:one",
      panels: [panel("one")],
      layout: { type: "leaf", id: "one" },
    },
    {
      id: "local",
      name: "Local",
      cwd: "/b",
      connection: "stale",
      panels: [panel("two")],
      layout: { type: "leaf", id: "two" },
    },
  ];
  const saved = restore({
    read: () => JSON.stringify({ workspaces, socket: "ssh:fallback" }),
  });
  assert.equal(saved.workspaces[0].connection, "ssh:one");
  assert.equal(saved.workspaces[1].connection, undefined);
});

test("restore returns null for missing, malformed, or incomplete storage", async () => {
  const { restore } = await library;
  assert.equal(restore({ read: () => null }), null);
  assert.equal(restore({ read: () => "{" }), null);
  assert.equal(
    restore({ read: () => JSON.stringify({ workspaces: [] }) }),
    null,
  );
  assert.equal(
    restore({ read: () => JSON.stringify({ workspaces: "bad" }) }),
    null,
  );
  assert.equal(
    restore({
      read: () => {
        throw new Error("storage unavailable");
      },
    }),
    null,
  );
});

test("saveWorkspaceState writes the serialized snapshot through the store and propagates its errors", async () => {
  const { saveWorkspaceState, flushWorkspaceState } = await library;
  const value = {
    workspaces: [],
    activeId: "active",
    socket: "local",
    routines: [],
    fontScale: 1,
  };
  const writes = [];
  const store = {
    write: (text) => writes.push(["write", text]),
    flush: (text) => writes.push(["flush", text]),
  };
  saveWorkspaceState(value, store);
  flushWorkspaceState(value, store);
  assert.deepEqual(
    writes.map(([kind]) => kind),
    ["write", "flush"],
  );
  for (const [, text] of writes) assert.deepEqual(JSON.parse(text), value);
  const failing = {
    write: () => {
      throw new Error("quota");
    },
    flush: () => {
      throw new Error("quota");
    },
  };
  assert.throws(() => saveWorkspaceState(value, failing), /quota/);
  assert.throws(() => flushWorkspaceState(value, failing), /quota/);
});

test("snapshotStore uses the bridge when there is one and localStorage under a new key otherwise", async () => {
  const { snapshotStore, BROWSER_KEY } = await library;
  const bridgeCalls = [];
  globalThis.window = {
    bridge: {
      workspaceStateRead: () => "from-file",
      workspaceStateWrite: async (text) => bridgeCalls.push(["write", text]),
      workspaceStateFlush: (text) => bridgeCalls.push(["flush", text]),
    },
  };
  const items = new Map();
  globalThis.localStorage = {
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => items.set(key, value),
    removeItem: (key) => items.delete(key),
  };
  try {
    const desktop = snapshotStore();
    assert.equal(desktop.read(), "from-file");
    await desktop.write("a");
    desktop.flush("b");
    assert.deepEqual(bridgeCalls, [
      ["write", "a"],
      ["flush", "b"],
    ]);
    assert.equal(items.size, 0, "the bridge never touches localStorage");

    delete globalThis.window;
    const web = snapshotStore();
    assert.equal(web.read(), null);
    web.write("c");
    assert.equal(items.get(BROWSER_KEY), "c");
    assert.equal(web.read(), "c");
    web.flush("d");
    assert.equal(web.read(), "d");
    assert.ok(!BROWSER_KEY.startsWith("sushiai.v1"));
  } finally {
    delete globalThis.window;
    delete globalThis.localStorage;
  }
});

test("restore accepts only a well-formed closedProjects array", async () => {
  const { restore } = await library;
  const workspaces = [
    {
      id: "w",
      name: "w",
      cwd: "/a",
      panels: [],
      layout: { type: "leaf", id: "w" },
    },
  ];
  const good = {
    id: "closed:ssh:devbox:/home/dev/app",
    name: "app",
    cwd: "/home/dev/app",
    endpoint: "ssh:devbox",
    herdr: true,
    closedAt: 1700000000000,
    git: {
      remote: "example.test/team/app",
      commonDir: "/home/dev/app/.git",
      checkout: "/home/dev/app",
      subdir: "",
      branch: "main",
    },
  };
  const saved = restore({
    read: () =>
      JSON.stringify({
        workspaces,
        socket: "local",
        closedProjects: [
          good,
          { name: "no id or cwd" },
          "not even an object",
          null,
        ],
      }),
  });
  assert.equal(saved.closedProjects.length, 1);
  assert.deepEqual(saved.closedProjects[0], good);

  const missingArray = restore({
    read: () => JSON.stringify({ workspaces, socket: "local" }),
  });
  assert.deepEqual(missingArray.closedProjects, []);

  const notAnArray = restore({
    read: () =>
      JSON.stringify({ workspaces, socket: "local", closedProjects: "bad" }),
  });
  assert.deepEqual(notAnArray.closedProjects, []);
});

test("initialWorkspace and codePanels preserve layout and panel identity invariants", async () => {
  const { initialWorkspace, codePanels } = await library;
  const workspace = initialWorkspace("/tmp/project");
  assert.equal(workspace.cwd, "/tmp/project");
  assert.equal(workspace.panels.length, 4);
  assert.equal(new Set(workspace.panels.map((item) => item.id)).size, 4);
  assert.equal(workspace.layout.axis, "row");
  assert.equal(workspace.layout.ratio, 0.53);
  assert.equal(workspace.layout.b.axis, "column");
  assert.equal(workspace.layout.b.ratio, 0.28);
  assert.equal(workspace.layout.b.b.axis, "column");
  assert.equal(workspace.layout.b.b.ratio, 0.49);
  assert.equal(codePanels(workspace).length, 4);
  const withoutCodeChat = { ...workspace, layout: null };
  assert.equal(codePanels(withoutCodeChat).length, 3);
  assert.equal(codePanels(withoutCodeChat)[0], workspace.panels[0]);
});
