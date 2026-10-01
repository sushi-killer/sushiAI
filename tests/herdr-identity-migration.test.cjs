const { test } = require("node:test");
const assert = require("node:assert/strict");

const state = import("../src/workspaceState.ts");
const snapshot = import("../src/herdrSnapshot.ts");
const leaf = (id) => ({ type: "leaf", id });
const split = (a, b) => ({
  type: "split",
  id: "split-original",
  axis: "row",
  ratio: 0.37,
  a: leaf(a),
  b: leaf(b),
});
const oldWorkspace = (endpoint, extra = {}) => ({
  id: "herdr:local:workspace-1",
  herdrId: "workspace-1",
  connection: endpoint,
  name: "App",
  cwd: "/work/app",
  panels: [
    {
      id: "herdr:local:pane-1",
      herdrId: "pane-1",
      kind: "terminal",
      title: "Shell",
      modelProfileId: "profile-1",
    },
    { id: `chat-${endpoint}`, kind: "chat", title: "Thread", messages: [] },
    {
      id: `extension-${endpoint}`,
      kind: "extension",
      title: "Ledger",
      extension: {
        extensionId: "test.probe",
        contributionId: "ledger",
        instanceId: "instance-1",
        stateVersion: 1,
      },
    },
  ],
  layout: split("herdr:local:pane-1", `extension-${endpoint}`),
  ...extra,
});
const storeFor = (value) => {
  const store = {
    text: JSON.stringify(value),
    writes: [],
    read: () => store.text,
    flush: (text) => {
      store.writes.push(text);
      store.text = text;
    },
  };
  return store;
};

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

test("restore migrates every ID reference once and preserves local and conversation state", async () => {
  const { restore } = await state;
  const endpoint = "/tmp/first.sock";
  const original = oldWorkspace(endpoint, {
    checkoutMetadata: { custom: true },
  });
  const local = {
    id: "local-workspace",
    cwd: "/work/local",
    name: "Local",
    panels: [{ id: "local-pane", kind: "terminal", title: "zsh" }],
    layout: leaf("local-pane"),
  };
  const store = storeFor({
    workspaces: [original, local],
    activeId: original.id,
    socket: endpoint,
    routines: [],
    fontScale: 1,
    selected: original.panels[0].id,
    zoomed: original.panels[0].id,
    chatFocus: original.panels[1].id,
    views: { [original.id]: { tabMode: true, zoomed: original.panels[0].id } },
    mergedLayouts: { merged: split(original.panels[0].id, "local-pane") },
    agentTabs: [
      { providerId: "p", agentId: "a", conversationId: "c", title: "T" },
    ],
    agentFocus: { providerId: "p", agentId: "a", active: "conversation-c" },
  });
  const restored = restore(store);
  const workspace = restored.workspaces[0];
  const terminalId = workspace.panels[0].id;
  assert.notEqual(workspace.id, original.id);
  assert.notEqual(terminalId, original.panels[0].id);
  assert.equal(restored.activeId, workspace.id);
  assert.equal(restored.selected, terminalId);
  assert.equal(restored.zoomed, terminalId);
  assert.equal(workspace.layout.a.id, terminalId);
  assert.equal(workspace.layout.id, "split-original");
  assert.equal(workspace.layout.ratio, 0.37);
  assert.equal(restored.views[workspace.id].zoomed, terminalId);
  assert.equal(restored.mergedLayouts.merged.a.id, terminalId);
  assert.equal(restored.mergedLayouts.merged.b.id, "local-pane");
  assert.equal(restored.chatFocus, original.panels[1].id);
  assert.deepEqual(workspace.panels[2].extension, original.panels[2].extension);
  assert.deepEqual(workspace.checkoutMetadata, { custom: true });
  assert.equal(restored.workspaces[1].id, "local-workspace");
  assert.deepEqual(restored.agentTabs, JSON.parse(store.text).agentTabs);
  assert.equal(restored.agentFocus.active, "conversation-c");
  assert.equal(store.writes.length, 1);
  assert.deepEqual(restore(store), restored);
  assert.equal(
    store.writes.length,
    1,
    "the second restore does not migrate again",
  );
});

test("colliding legacy IDs use owner contexts without dropping either endpoint", async () => {
  const { restore } = await state;
  const first = oldWorkspace("/tmp/first.sock");
  const second = oldWorkspace("/tmp/second.sock");
  const oldPane = first.panels[0].id;
  const store = storeFor({
    workspaces: [first, second],
    activeId: first.id,
    socket: second.connection,
    selected: oldPane,
    zoomed: oldPane,
    views: { [first.id]: { tabMode: true, zoomed: oldPane } },
    mergedLayouts: { merged: split(oldPane, oldPane) },
  });
  const restored = restore(store);
  const [a, b] = restored.workspaces;
  assert.equal(restored.workspaces.length, 2);
  assert.notEqual(a.id, b.id);
  assert.equal(
    restored.activeId,
    b.id,
    "the saved endpoint identifies the active owner",
  );
  assert.equal(restored.selected, b.panels[0].id);
  assert.equal(restored.zoomed, b.panels[0].id);
  assert.equal(a.layout.a.id, a.panels[0].id);
  assert.equal(b.layout.a.id, b.panels[0].id);
  assert.equal(restored.views[a.id].zoomed, a.panels[0].id);
  assert.equal(restored.views[b.id].zoomed, b.panels[0].id);
  assert.deepEqual(
    [restored.mergedLayouts.merged.a.id, restored.mergedLayouts.merged.b.id],
    [a.panels[0].id, b.panels[0].id],
  );
});

test("a failed migration write still restores and retries on next startup", async () => {
  const { restore } = await state;
  const store = storeFor({ workspaces: [oldWorkspace("/tmp/first.sock")] });
  store.flush = () => {
    throw new Error("disk full");
  };
  const first = restore(store);
  assert.ok(first);
  assert.notEqual(first.workspaces[0].id, "herdr:local:workspace-1");
  assert.deepEqual(restore(store), first);
});

test("migration restores endpoint ownership, closed-project references and SSH panels", async () => {
  const { restore } = await state;
  const endpoint = "ssh:devbox";
  const original = oldWorkspace(endpoint);
  delete original.connection;
  const closed = {
    id: "closed:local:/work/app",
    name: "App",
    cwd: "/work/app",
    endpoint: "/tmp/first.sock",
    herdr: true,
    closedAt: 1,
    git: { remote: "", commonDir: "", checkout: "", subdir: "", branch: "" },
  };
  const store = storeFor({
    workspaces: [original],
    activeId: original.id,
    socket: endpoint,
    closedProjects: [closed],
    views: { [closed.id]: { tabMode: true, zoomed: null } },
    chatFocus: original.panels[0].id,
  });
  const restored = restore(store);
  assert.equal(restored.workspaces[0].connection, endpoint);
  assert.equal(restored.workspaces[0].id, "herdr:v2:ssh%3Adevbox:workspace-1");
  assert.equal(
    restored.workspaces[0].panels[0].id,
    "herdr:v2:ssh%3Adevbox:pane-1",
  );
  assert.equal(restored.chatFocus, restored.workspaces[0].panels[0].id);
  assert.equal(
    restored.closedProjects[0].id,
    "closed:/tmp/first.sock:/work/app",
  );
  assert.deepEqual(restored.views[restored.closedProjects[0].id], {
    tabMode: true,
    zoomed: null,
  });
});

test("a rebound Herdr workspace keeps its already-migrated persistent identity", async () => {
  const { restore } = await state;
  const workspace = oldWorkspace("/tmp/first.sock", {
    id: "herdr:v2:%2Ftmp%2Ffirst.sock:original-workspace",
    herdrId: "replacement-workspace",
  });
  workspace.panels[0].id = "herdr:v2:%2Ftmp%2Ffirst.sock:pane-1";
  workspace.layout = split(workspace.panels[0].id, workspace.panels[2].id);
  const store = storeFor({
    workspaces: [workspace],
    activeId: workspace.id,
    socket: workspace.connection,
  });
  const restored = restore(store);
  assert.equal(restored.workspaces[0].id, workspace.id);
  assert.equal(restored.activeId, workspace.id);
  assert.equal(store.writes.length, 0);
});
