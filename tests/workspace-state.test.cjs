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
  const { restore, STORAGE } = await library;
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
  const saved = restore({ getItem: (key) => (key === STORAGE ? data : null) });
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
    getItem: () => JSON.stringify({ workspaces, socket: "ssh:fallback" }),
  });
  assert.equal(saved.workspaces[0].connection, "ssh:one");
  assert.equal(saved.workspaces[1].connection, undefined);
});

test("restore returns null for missing, malformed, or incomplete storage", async () => {
  const { restore } = await library;
  assert.equal(restore({ getItem: () => null }), null);
  assert.equal(restore({ getItem: () => "{" }), null);
  assert.equal(
    restore({ getItem: () => JSON.stringify({ workspaces: [] }) }),
    null,
  );
  assert.equal(
    restore({ getItem: () => JSON.stringify({ workspaces: "bad" }) }),
    null,
  );
  assert.equal(
    restore({
      getItem: () => {
        throw new Error("storage unavailable");
      },
    }),
    null,
  );
});

test("saveWorkspaceState writes the stable key and propagates storage errors", async () => {
  const { saveWorkspaceState, STORAGE } = await library;
  const value = {
    workspaces: [],
    activeId: "active",
    socket: "local",
    routines: [],
    fontScale: 1,
  };
  let write;
  saveWorkspaceState(value, {
    setItem: (key, payload) => {
      write = { key, payload };
    },
  });
  assert.equal(write.key, STORAGE);
  assert.deepEqual(JSON.parse(write.payload), value);
  assert.throws(
    () =>
      saveWorkspaceState(value, {
        setItem: () => {
          throw new Error("quota");
        },
      }),
    /quota/,
  );
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
