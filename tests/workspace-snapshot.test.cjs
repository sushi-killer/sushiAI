const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const library = import("../src/workspaceState.ts");
const helper = require("../electron/workspace-snapshot.cjs");
const { appDb, closeAppDb } = require("../electron/app-db.cjs");

const leaf = (id) => ({ type: "leaf", id });
const split = (id, axis, ratio, a, b) => ({
  type: "split",
  id,
  axis,
  ratio,
  a,
  b,
});
const messageStore = () => {
  const store = {
    text: null,
    read: () => store.text,
    write: (text) => {
      store.text = text;
    },
    flush: (text) => {
      store.text = text;
    },
  };
  return store;
};
const runtime = { busy: false, started: false };

function busySnapshot() {
  const panels = [
    { id: "local-term", kind: "terminal", title: "zsh", ...runtime },
    {
      id: "session:pane-1",
      kind: "terminal",
      title: "zsh",
      sessionId: "pane-1",
      status: "idle",
      ...runtime,
    },
    {
      id: "local-agent",
      kind: "agent",
      title: "Codex",
      agent: "codex",
      modelProfileId: "profile-1",
      ...runtime,
    },
    {
      id: "session:pane-2",
      kind: "agent",
      title: "Claude Code",
      agent: "claude",
      sessionId: "pane-2",
      status: "working",
      ...runtime,
    },
    {
      id: "browser",
      kind: "browser",
      title: "Browser",
      url: "http://127.0.0.1:5173/docs?tab=2",
      ...runtime,
    },
    {
      id: "files",
      kind: "files",
      title: "Files & Git",
      filesTarget: { root: "/work/app", path: "src/a.ts" },
      filesView: {
        root: "/work/app",
        directory: "src/sub",
        file: "src/sub/b.ts",
      },
      ...runtime,
    },
    {
      id: "chat",
      kind: "chat",
      title: "Thread",
      agent: "claude",
      messages: [
        { id: "m1", role: "user", text: "hello" },
        { id: "m2", role: "assistant", text: "hi", model: "sonnet" },
      ],
      ...runtime,
    },
    {
      id: "orch",
      kind: "extension",
      title: "Orchestrator",
      extension: {
        extensionId: "builtin.orchestrator",
        contributionId: "orchestration",
        instanceId: "orch",
        stateVersion: 1,
        args: {
          view: '{"kind":"task","id":"task-7"}',
          host: "ssh:lab",
          repo: "/work/repo",
        },
      },
      ...runtime,
    },
    {
      id: "ext",
      kind: "extension",
      title: "Probe",
      extension: {
        extensionId: "test.probe",
        contributionId: "probe.pane",
        instanceId: "i-1",
        stateVersion: 1,
      },
      ...runtime,
    },
    {
      id: "session:pane-9",
      kind: "terminal",
      title: "zsh",
      sessionId: "pane-9",
      ended: true,
      ...runtime,
    },
  ];
  const layout = split(
    "s1",
    "row",
    0.37,
    leaf("local-term"),
    split(
      "s2",
      "column",
      0.61,
      split("s3", "row", 0.29, leaf("session:pane-1"), leaf("local-agent")),
      split(
        "s4",
        "column",
        0.44,
        split("s5", "row", 0.71, leaf("session:pane-2"), leaf("browser")),
        split(
          "s6",
          "row",
          0.18,
          leaf("files"),
          split(
            "s7",
            "column",
            0.82,
            leaf("orch"),
            split("s8", "row", 0.33, leaf("ext"), leaf("session:pane-9")),
          ),
        ),
      ),
    ),
  );
  return {
    workspaces: [
      {
        id: "w-local",
        name: "app",
        cwd: "/work/app",
        connection: "local",
        panels,
        layout,
      },
      {
        id: "session:w1",
        name: "remote",
        cwd: "/work/remote",
        connection: "local",
        panels: [
          {
            id: "solo",
            kind: "terminal",
            title: "zsh",
            sessionId: "s-solo",
            ...runtime,
          },
        ],
        layout: leaf("solo"),
      },
    ],
    activeId: "w-local",
    routines: [{ id: "r1", name: "Test", command: "npm test" }],
    fontScale: 1.15,
    mode: "Agent",
    tabMode: true,
    section: "Skills",
    selected: "files",
    zoomed: "browser",
    sidebar: false,
    route: {
      kind: "extension",
      surfaceId: "extension.test.probe.probe.ledger",
      presentation: "section",
      extensionId: "test.probe",
      targetSurfaceId: "probe.ledger",
    },
    workspaceGrouping: "flat",
    closedProjects: [],
    views: { "w-local": { tabMode: true, zoomed: "browser" } },
    mergedLayouts: {
      "group-1": split(
        "g1",
        "column",
        0.4,
        leaf("local-term"),
        leaf("session:pane-1"),
      ),
    },
    agentTabs: [
      {
        providerId: "hermes",
        agentId: "a",
        conversationId: "c1",
        title: "One",
      },
      {
        providerId: "hermes",
        agentId: "a",
        conversationId: "c2",
        title: "Two",
      },
    ],
    agentFocus: {
      providerId: "hermes",
      agentId: "a",
      active: '["hermes","a","c2"]',
    },
    chatFocus: "chat",
  };
}

test("a busy snapshot round-trips through the store, deep-equal", async () => {
  const { saveWorkspaceState, restore } = await library;
  const snapshot = busySnapshot();
  const store = messageStore();
  saveWorkspaceState(snapshot, store);
  const restored = restore(store);
  assert.deepEqual(JSON.parse(JSON.stringify(restored)), snapshot);
  // Ratios are the point: a default would be 0.5.
  assert.equal(restored.workspaces[0].layout.ratio, 0.37);
  assert.equal(restored.workspaces[0].panels[5].filesView.directory, "src/sub");
  assert.deepEqual(
    JSON.parse(restored.workspaces[0].panels[7].extension.args.view),
    { kind: "task", id: "task-7" },
  );
});

test("restore resets what only makes sense while a process runs", async () => {
  const { saveWorkspaceState, restore } = await library;
  const snapshot = busySnapshot();
  snapshot.workspaces[0].panels[2] = {
    ...snapshot.workspaces[0].panels[2],
    busy: true,
    started: true,
  };
  const store = messageStore();
  saveWorkspaceState(snapshot, store);
  const agent = restore(store).workspaces[0].panels[2];
  assert.equal(agent.busy, false);
  assert.equal(agent.started, false, "a local agent shows Start again");
});

test("agent tabs are capped at 30 and malformed rows are dropped", async () => {
  const { restore } = await library;
  const tab = (n) => ({
    providerId: "p",
    agentId: "a",
    conversationId: `c${n}`,
    title: `t${n}`,
  });
  const workspace = { id: "w", name: "w", cwd: "/", panels: [], layout: null };
  const restored = restore({
    read: () =>
      JSON.stringify({
        workspaces: [workspace],
        socket: "",
        agentTabs: [
          ...Array.from({ length: 40 }, (_, n) => tab(n)),
          { providerId: "p" },
          null,
        ],
        agentFocus: { providerId: 1 },
        chatFocus: 7,
      }),
  });
  assert.equal(restored.agentTabs.length, 30);
  assert.equal(restored.agentTabs.at(-1).conversationId, "c39");
  assert.deepEqual(restored.agentFocus, {
    providerId: "",
    agentId: "",
    active: "",
  });
  assert.equal(restored.chatFocus, "");
});

function legacyStorage(entries) {
  const items = new Map(Object.entries(entries));
  return {
    items,
    getItem: (key) => items.get(key) ?? null,
    removeItem: (key) => items.delete(key),
  };
}

test("legacy localStorage keys fold into one snapshot, once, and are deleted", async () => {
  const { restore } = await library;
  const workspace = {
    id: "w",
    name: "old",
    cwd: "/old",
    panels: [{ id: "p", kind: "terminal", title: "zsh" }],
    layout: leaf("p"),
  };
  const merged = { "group-1": split("g", "row", 0.3, leaf("a"), leaf("b")) };
  const tabs = [
    { providerId: "p", agentId: "a", conversationId: "c", title: "T" },
  ];
  const focus = { providerId: "p", agentId: "a", active: "x" };
  const legacy = legacyStorage({
    "sushiai.v1": JSON.stringify({
      workspaces: [workspace],
      activeId: "w",
      socket: "s",
      routines: [],
      fontScale: 1,
      mode: "Chat",
    }),
    "sushiai.mergedLayouts.v1": JSON.stringify(merged),
    "sushiai.agent-tabs.v1": JSON.stringify(tabs),
    "sushiai.agent-focus.v1": JSON.stringify(focus),
    "sushiai.chat-focus.v1": "thread-1",
    "sushiai.keep-awake": "true",
  });
  const writes = [];
  const store = {
    read: () => null,
    write: () => assert.fail("the import writes synchronously"),
    flush: (text) => writes.push(text),
    legacy,
  };
  const restored = restore(store);
  assert.equal(restored.mode, "Chat");
  assert.equal(restored.workspaces[0].name, "old");
  assert.deepEqual(restored.mergedLayouts, merged);
  assert.deepEqual(restored.agentTabs, tabs);
  assert.deepEqual(restored.agentFocus, focus);
  assert.equal(restored.chatFocus, "thread-1");
  assert.equal(writes.length, 1, "one snapshot is written");
  assert.deepEqual(JSON.parse(writes[0]).agentTabs, tabs);
  assert.deepEqual(
    [...legacy.items.keys()],
    ["sushiai.keep-awake"],
    "only the five legacy keys are deleted",
  );
  // Second start: the snapshot exists, nothing is imported again.
  store.read = () => writes[0];
  legacy.items.set("sushiai.v1", JSON.stringify({ workspaces: [workspace] }));
  assert.equal(restore(store).mode, "Chat");
  assert.equal(writes.length, 1);
  assert.ok(legacy.items.has("sushiai.v1"), "an existing snapshot wins");
});

test("a failed legacy write keeps the old keys for the next start", async () => {
  const { restore } = await library;
  const workspace = { id: "w", name: "w", cwd: "/", panels: [], layout: null };
  const legacy = legacyStorage({
    "sushiai.v1": JSON.stringify({ workspaces: [workspace] }),
  });
  const restored = restore({
    read: () => null,
    write: () => {},
    flush: () => {
      throw new Error("disk full");
    },
    legacy,
  });
  assert.equal(restored.workspaces.length, 1, "this session still restores");
  assert.ok(legacy.items.has("sushiai.v1"));
});

test("orphaned legacy keys are cleared even when sushiai.v1 is missing", async () => {
  const { restore } = await library;
  const legacy = legacyStorage({
    "sushiai.agent-tabs.v1": "[]",
    "sushiai.chat-focus.v1": "c1",
  });
  assert.equal(
    restore({ read: () => null, write() {}, flush() {}, legacy }),
    null,
  );
  assert.equal(legacy.items.size, 0);
});

test("without a snapshot or legacy data restore returns null", async () => {
  const { restore } = await library;
  assert.equal(
    restore({
      read: () => null,
      write() {},
      flush() {},
      legacy: legacyStorage({}),
    }),
    null,
  );
});

test("the electron helper stores row deltas and reads back", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-test-"));
  try {
    assert.equal(helper.readSnapshot(dir), null);
    helper.writeSnapshotSync(dir, '{"a":1}');
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ a: 1 },
      workspaces: [],
    });
    helper.writeSnapshotSync(dir, '{"a":2}');
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ a: 2 },
      workspaces: [],
    });

    // A failed write leaves the previous snapshot whole.
    assert.throws(() => helper.writeSnapshotSync(dir, undefined), /Invalid/);
    assert.throws(() => helper.writeSnapshotSync(dir, "{broken"));
    assert.throws(() => helper.writeSnapshotSync(dir, "[1]"), /Invalid/);
    assert.throws(
      () => helper.writeSnapshotSync(dir, '{"a":3,"workspaces":5}'),
      /Invalid/,
    );
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ a: 2 },
      workspaces: [],
    });

    // A key missing from the next snapshot is deleted.
    helper.writeSnapshotSync(dir, '{"b":true}');
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ b: true },
      workspaces: [],
    });
  } finally {
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

const ws = (id, extra = {}) => ({ id, name: id, ...extra });

test("an unreadable stored row is skipped and workspaces is always emitted", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-test-"));
  const warn = console.warn;
  console.warn = () => {};
  try {
    helper.writeSnapshotSync(dir, '{"a":1,"b":2}');
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      a: 1,
      b: 2,
      workspaces: [],
    });
    appDb(dir)
      .prepare("UPDATE app_state SET value = '{x' WHERE key = 'a'")
      .run();
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      b: 2,
      workspaces: [],
    });
  } finally {
    console.warn = warn;
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("workspaces round trip in order and a write touches only the changed row", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-test-"));
  try {
    const first = {
      activeId: "a",
      views: { a: 1 },
      workspaces: [
        ws("a", { connection: "ssh:devbox" }),
        ws("b"),
        ws("c", { connection: "ssh:devbox" }),
      ],
    };
    helper.writeSnapshotSync(dir, JSON.stringify(first));
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), first);
    assert.deepEqual(
      helper.savedWorkspaces(dir).map((item) => item.id),
      ["a", "b", "c"],
    );

    const db = appDb(dir);
    const total = () => db.prepare("SELECT total_changes() AS n").get().n;
    const before = total();
    helper.writeSnapshotSync(dir, JSON.stringify(first));
    assert.equal(total(), before, "an identical write changes nothing");

    const next = structuredClone(first);
    next.workspaces[1].name = "renamed";
    helper.writeSnapshotSync(dir, JSON.stringify(next));
    assert.equal(total() - before, 1, "one workspace row, nothing else");
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), next);

    // Reordering moves positions; removing deletes the row.
    next.workspaces = [next.workspaces[2], next.workspaces[0]];
    helper.writeSnapshotSync(dir, JSON.stringify(next));
    assert.deepEqual(
      helper.savedWorkspaces(dir).map((item) => item.id),
      ["c", "a"],
    );
    assert.equal(db.prepare("SELECT count(*) AS n FROM workspaces").get().n, 2);
  } finally {
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("a duplicate id keeps the first row; rows are keyed by id, not by a host binding", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-test-"));
  try {
    helper.writeSnapshotSync(
      dir,
      JSON.stringify({
        workspaces: [
          ws("a", { connection: "ssh:devbox" }),
          ws("b", { connection: "ssh:devbox" }),
          ws("a", { name: "again" }),
          ws("c", { connection: "ssh:other" }),
          ws("d"),
          ws("e"),
        ],
      }),
    );
    assert.deepEqual(
      helper.savedWorkspaces(dir).map((item) => [item.id, item.name]),
      [
        ["a", "a"],
        ["b", "b"],
        ["c", "c"],
        ["d", "d"],
        ["e", "e"],
      ],
    );
    const rows = appDb(dir)
      .prepare("SELECT id, endpoint FROM workspaces ORDER BY position")
      .all()
      .map((row) => [row.id, row.endpoint]);
    assert.deepEqual(rows, [
      ["a", "ssh:devbox"],
      ["b", "ssh:devbox"],
      ["c", "ssh:other"],
      ["d", ""],
      ["e", ""],
    ]);
    // A panel's session binding lives in the row's data and survives a write.
    const bound = ws("a", {
      connection: "ssh:devbox",
      panels: [{ id: "p", kind: "terminal", title: "zsh", sessionId: "s-1" }],
    });
    helper.writeSnapshotSync(dir, JSON.stringify({ workspaces: [bound] }));
    assert.equal(helper.savedWorkspaces(dir)[0].panels[0].sessionId, "s-1");
    assert.equal(helper.savedWorkspaces(dir).length, 1);
  } finally {
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

function fakeIpc(trustedSender) {
  const window = { webContents: { mainFrame: {} } };
  const trusted = {
    sender: window.webContents,
    senderFrame: window.webContents.mainFrame,
  };
  const sync = new Map();
  const invoke = new Map();
  return {
    window,
    trusted,
    ipcMain: { on: (channel, fn) => sync.set(channel, fn) },
    handle: (channel, fn) => invoke.set(channel, fn),
    send(channel, event, ...args) {
      const reply = { returnValue: undefined };
      sync.get(channel)(
        {
          ...event,
          get returnValue() {
            return reply.returnValue;
          },
          set returnValue(v) {
            reply.returnValue = v;
          },
        },
        ...args,
      );
      return reply.returnValue;
    },
    invoke: (channel, ...args) => invoke.get(channel)(...args),
    trustedSender,
  };
}

test("the electron helper serves read, write and flush and refuses other senders", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-ipc-"));
  try {
    const ipc = fakeIpc();
    helper.registerWorkspaceSnapshot({
      ipcMain: ipc.ipcMain,
      handle: ipc.handle,
      getMainWindow: () => ipc.window,
      userDataDir: () => dir,
    });
    assert.deepEqual(ipc.send("workspace-state-read", ipc.trusted), {
      value: null,
    });
    await ipc.invoke("workspace-state-write", '{"v":"one"}');
    assert.deepEqual(ipc.send("workspace-state-read", ipc.trusted), {
      value: '{"v":"one","workspaces":[]}',
    });
    assert.deepEqual(
      ipc.send("workspace-state-flush", ipc.trusted, '{"v":"two"}'),
      {
        value: null,
      },
    );
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ v: "two" },
      workspaces: [],
    });

    // A flush overtakes a write still waiting: the older text never lands.
    const pending = ipc.invoke("workspace-state-write", '{"v":"old"}');
    ipc.send("workspace-state-flush", ipc.trusted, '{"v":"newest"}');
    await pending;
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ v: "newest" },
      workspaces: [],
    });

    const stranger = { sender: {}, senderFrame: {} };
    assert.match(
      ipc.send("workspace-state-flush", stranger, '{"v":"evil"}').error,
      /Untrusted/,
    );
    assert.match(ipc.send("workspace-state-read", stranger).error, /Untrusted/);
    assert.deepEqual(JSON.parse(helper.readSnapshot(dir)), {
      ...{ v: "newest" },
      workspaces: [],
    });
    assert.match(
      ipc.send("workspace-state-flush", ipc.trusted, 42).error,
      /Invalid/,
    );
  } finally {
    closeAppDb(dir);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("empty session workspaces survive restore while ended-only rows move to Recently closed", async () => {
  const { restore } = await library;
  const host = (id, cwd, panels, connection = "local") => ({
    id,
    name: cwd.split("/").pop(),
    cwd,
    connection,
    panels,
    layout: panels.length ? leaf(panels[0].id) : null,
  });
  const pane = (id, extra = {}) => ({
    id,
    kind: "terminal",
    title: id,
    sessionId: `s-${id}`,
    ...extra,
  });
  // What a few host restarts left: a live project, its dropped worktree, an
  // intentionally empty row, and a dropped row with a chat.
  const workspaces = [
    host("w1", "/repo/app", [pane("p1")]),
    host("w2", "/repo/app-wt", [pane("p2", { ended: true })]),
    host("w3", "/repo/app", [], "ssh:host"),
    host("w4", "/repo/notes", [
      pane("p4", { ended: true }),
      { id: "c4", kind: "chat", title: "chat" },
    ]),
  ];
  const store = messageStore();
  store.text = JSON.stringify({
    workspaces,
    closedProjects: [],
    activeId: workspaces[1].id,
    selected: "p2",
  });
  const restored = restore(store);
  assert.equal(restored.activeId, workspaces[0].id, "no active swept row");
  assert.equal(restored.selected, "");
  assert.deepEqual(
    restored.workspaces.map((w) => w.id),
    ["w1", "w3", "w4"],
    "the empty project and the one holding a chat stay in the workspace list",
  );
  assert.deepEqual(
    restored.closedProjects.map((p) => [p.cwd, p.endpoint, p.backed]),
    [["/repo/app-wt", "local", true]],
  );
  assert.deepEqual(
    JSON.parse(store.text).workspaces.map((w) => w.id),
    ["w1", "w3", "w4"],
    "the ended-only row is swept while the empty project stays saved",
  );

  const lone = messageStore();
  lone.text = JSON.stringify({ workspaces: [workspaces[2]] });
  assert.equal(
    restore(lone).workspaces.length,
    1,
    "the only row stays rather than leaving an empty list",
  );
});

test("a panel that ran without a session restores ended and unbound; a session panel stays live", async () => {
  const { restore } = await library;
  const store = messageStore();
  store.text = JSON.stringify({
    workspaces: [
      {
        id: "w1",
        name: "app",
        cwd: "/repo/app",
        panels: [
          {
            id: "legacy",
            kind: "agent",
            title: "Claude",
            status: "idle",
            paneCwd: "/repo/app",
          },
          {
            id: "bound",
            kind: "terminal",
            title: "zsh",
            sessionId: "s-1",
            started: true,
          },
          { id: "chat", kind: "chat", title: "Thread" },
        ],
        layout: leaf("legacy"),
      },
    ],
  });
  const [workspace] = restore(store).workspaces;
  const [legacy, bound, chat] = workspace.panels;
  assert.equal(legacy.ended, true, "no session is adopted");
  assert.equal(legacy.sessionId, undefined);
  assert.equal(bound.ended, undefined, "reconcile decides, not restore");
  assert.equal(bound.sessionId, "s-1");
  assert.equal(bound.started, false);
  assert.equal(chat.ended, undefined);
  assert.equal(workspace.connection, "local");
});
