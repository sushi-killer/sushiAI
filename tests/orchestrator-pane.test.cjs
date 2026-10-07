const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  ORCHESTRATOR_MANIFEST,
} = require("../electron/extensions/builtin-orchestrator.cjs");
const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");

const leaf = (id) => ({ type: "leaf", id });
const pane = (id) => ({
  id,
  kind: "extension",
  title: "Orchestrator",
  extension: {
    extensionId: "builtin.orchestrator",
    contributionId: "orchestration",
    instanceId: id,
    stateVersion: 1,
  },
});
const workspace = (id, cwd, panels = []) => ({
  id,
  name: id,
  cwd,
  panels,
  layout: panels.length ? leaf(panels[0].id) : null,
});

async function orchestratorRegistry() {
  const { ExtensionRegistry } = await import("../src/extensions/registry.ts");
  const manifest = validateExtensionManifest(ORCHESTRATOR_MANIFEST);
  const registry = new ExtensionRegistry();
  registry.applySnapshot({
    schemaVersion: 2,
    version: 1,
    extensions: [{ manifest, status: "active" }],
    surfaces: manifest.contributions.surfaces,
    navigation: manifest.contributions.navigation,
    actions: [],
    commands: [],
  });
  return registry;
}

test("the manifest validates and puts Orchestrator in the panel picker", async () => {
  const { ExtensionRegistry } = await import("../src/extensions/registry.ts");
  const { navigationFor } = await import("../src/extensions/routes.ts");
  const manifest = validateExtensionManifest(ORCHESTRATOR_MANIFEST);
  const surface = manifest.contributions.surfaces.find(
    (item) => item.id === "orchestration",
  );
  assert.deepEqual(surface.allowedHosts, ["workspace.pane"]);
  assert.equal(surface.instancePolicy, "multiple");
  assert.deepEqual(surface.view, {
    kind: "core",
    viewId: "orchestrator.panel",
  });
  const registry = new ExtensionRegistry();
  registry.applySnapshot({
    schemaVersion: 2,
    version: 1,
    extensions: [{ manifest, status: "active" }],
    surfaces: manifest.contributions.surfaces,
    navigation: manifest.contributions.navigation,
    actions: [],
    commands: [],
  });
  const entries = navigationFor(registry, "panel.picker");
  assert.deepEqual(
    entries.map((item) => [item.label, item.targetSurfaceId]),
    [["Orchestrator", "orchestration"]],
  );
  const created = registry.createPanel(
    "builtin.orchestrator",
    "orchestration",
    "p1",
  );
  assert.equal(created.extension.contributionId, "orchestration");
});

test("a saved orchestrator panel loads as the extension pane with its state", async () => {
  const { restore } = await import("../src/workspaceState.ts");
  // The shape the app wrote before the orchestrator became an extension pane.
  const saved = {
    activeId: "w1",
    socket: "/tmp/local.sock",
    workspaces: [
      {
        id: "w1",
        name: "repo",
        cwd: "/work/repo",
        layout: leaf("orch"),
        panels: [
          {
            id: "orch",
            kind: "orchestrator",
            title: "Orchestrator",
            orchestratorView: { kind: "task", id: "task-7" },
            orchestratorHost: "ssh:lab",
            orchestratorRepo: "/srv/repo",
          },
          {
            id: "plain",
            kind: "orchestrator",
            title: "Orchestrator",
          },
        ],
      },
    ],
  };
  const store = {
    read: () => JSON.stringify(saved),
    write() {},
    flush() {},
  };
  const [first, second] = restore(store).workspaces[0].panels;
  assert.equal(first.kind, "extension");
  assert.equal(first.id, "orch");
  assert.equal(first.title, "Orchestrator");
  assert.deepEqual(first.extension, {
    extensionId: "builtin.orchestrator",
    contributionId: "orchestration",
    instanceId: "orch",
    stateVersion: 1,
    args: {
      view: '{"kind":"task","id":"task-7"}',
      host: "ssh:lab",
      repo: "/srv/repo",
    },
  });
  for (const key of [
    "orchestratorView",
    "orchestratorHost",
    "orchestratorRepo",
  ])
    assert.equal(key in first, false, `${key} is gone`);
  assert.equal(second.kind, "extension");
  assert.equal(second.extension.args, undefined);
});

test("the pane's view args round-trip", async () => {
  // JSX is not loadable in node, so the pure helpers live in a .ts module.
  const { parseView } = await import("../src/orchestrator/paneArgs.ts");
  assert.deepEqual(parseView('{"kind":"task","id":"t"}'), {
    kind: "task",
    id: "t",
  });
  assert.equal(parseView("not json"), undefined);
  assert.equal(parseView(undefined), undefined);
});

test("two args writes in one tick both land: the view reset and the new host", async () => {
  const { extensionArgsUpdate, mergeArgs } =
    await import("../src/extensions/args.ts");
  const { patchPanel } = await import("../src/workspace/workspace-actions.ts");
  assert.deepEqual(mergeArgs({ view: "x", host: "h" }, { view: undefined }), {
    host: "h",
  });
  assert.deepEqual(mergeArgs(undefined, { host: "ssh:lab" }), {
    host: "ssh:lab",
  });
  const start = [
    workspace("w1", "/repo", [
      {
        ...pane("p1"),
        extension: {
          ...pane("p1").extension,
          args: { view: '{"kind":"task","id":"t"}', host: "old", repo: "/old" },
        },
      },
    ]),
  ];
  // Both writes were built from the same stale args, as in one event handler.
  const afterReset = patchPanel(
    start,
    "p1",
    extensionArgsUpdate({ view: undefined }),
  );
  const afterHost = patchPanel(
    afterReset,
    "p1",
    extensionArgsUpdate({ host: "ssh:lab", repo: "/r" }),
  );
  assert.deepEqual(afterHost[0].panels[0].extension.args, {
    host: "ssh:lab",
    repo: "/r",
  });
});

test("the open path reveals an existing pane, adds one, or creates a workspace", async () => {
  const { openStep } = await import("../src/orchestrator/moduleShell.ts");
  const { placeSurface } = await import("../src/extensions/useModuleShell.ts");
  const registry = await orchestratorRegistry();
  const withPane = workspace("w2", "/repo", [pane("p2")]);
  const plain = workspace("w1", "/repo", [
    { id: "t", kind: "terminal", title: "zsh" },
  ]);
  const other = workspace("w3", "/other");
  const target = { taskId: "t1", repo: "/repo", focus: "summary" };

  const reveal = openStep([other, plain, withPane], target);
  assert.deepEqual(reveal, { kind: "surface", workspaceId: "w2" });
  const placed = placeSurface(
    [other, plain, withPane],
    { workspaceId: reveal.workspaceId },
    registry,
    "builtin.orchestrator",
    "orchestration",
  );
  assert.equal(placed.kind, "reveal");
  assert.equal(placed.panel.id, "p2");

  const add = openStep([other, plain], target);
  assert.deepEqual(add, { kind: "surface", workspaceId: "w1" });
  assert.deepEqual(
    placeSurface(
      [other, plain],
      { workspaceId: "w1" },
      registry,
      "builtin.orchestrator",
      "orchestration",
    ),
    { kind: "add", workspaceId: "w1" },
  );

  const create = openStep([other], { ...target, repo: "/work/my-repo" });
  assert.deepEqual(create, {
    kind: "workspace",
    cwd: "/work/my-repo",
    connection: "local",
    name: "my-repo",
  });
  assert.deepEqual(
    placeSurface(
      [other],
      { cwd: create.cwd, connection: create.connection, name: create.name },
      registry,
      "builtin.orchestrator",
      "orchestration",
    ),
    {
      kind: "create",
      cwd: "/work/my-repo",
      connection: "local",
      name: "my-repo",
    },
  );
  assert.equal(
    placeSurface([other], { workspaceId: "gone" }, registry, "x", "y").kind,
    "missing",
  );
});

test("a local task never opens on a workspace of another host", async () => {
  const { openStep } = await import("../src/orchestrator/moduleShell.ts");
  const { placeSurface, queuedWorkspace } =
    await import("../src/extensions/useModuleShell.ts");
  const registry = await orchestratorRegistry();
  const remote = { ...workspace("w-ssh", "/repo", []), connection: "ssh:lab" };
  const target = { taskId: "t1", repo: "/repo", focus: "summary" };
  // The active workspace is on an SSH host and shares the repo path.
  const step = openStep([remote], target);
  assert.deepEqual(step, {
    kind: "workspace",
    cwd: "/repo",
    connection: "local",
    name: "repo",
  });
  const place = placeSurface(
    [remote],
    { cwd: step.cwd, connection: step.connection, name: step.name },
    registry,
    "builtin.orchestrator",
    "orchestration",
  );
  assert.equal(place.connection, "local");
  assert.equal(queuedWorkspace([remote], place), undefined);
  const created = workspace("w-new", "/repo");
  assert.equal(queuedWorkspace([remote, created], place), created);
  // A remote task keeps its host.
  assert.equal(
    openStep([], { ...target, host: "ssh:lab" }).connection,
    "ssh:lab",
  );
});
