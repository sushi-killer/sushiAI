const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  SessionLauncher,
  registerSessionLaunchIpc,
} = require("../electron/session-launch.cjs");
const { appDb, closeAppDb } = require("../electron/app-db.cjs");
const {
  savedWorkspaces,
  writeSnapshotSync,
} = require("../electron/workspace-snapshot.cjs");

const launch = (operationId, extra = {}) => ({
  operationId,
  endpoint: "/tmp/launch-herdr.sock",
  cwd: "/tmp/checkout",
  label: "Checkout",
  kind: "terminal",
  ...extra,
});

function setup(options = {}) {
  const calls = [];
  const workspaces = [];
  const panes = [];
  const connections = {
    socket: async (endpoint) => endpoint,
    exec: async (endpoint, command, extra = {}) => {
      calls.push({ endpoint, command, payload: extra.input });
      if (options.exec) return options.exec(endpoint, command, extra);
      return "/tmp/remote-session-env";
    },
    inspect: async (endpoint, input) => {
      calls.push({ endpoint, inspection: input });
      if (options.inspect) return options.inspect(endpoint, input);
      if (input.operation === "canonical_checkout")
        return {
          cwd: input.root.replace("/alias", "/checkout").replace(/\/$/, ""),
        };
      if (input.operation === "worktree_create")
        return { path: "/tmp/checkout-branch" };
      if (input.operation === "worktree_base")
        return { cwd: input.root, base: "prepared-commit" };
      throw new Error(`Unexpected inspection ${input.operation}`);
    },
  };
  const rpc = async (endpoint, method, params) => {
    calls.push({ endpoint, method, params });
    if (options.rpc) {
      const value = await options.rpc(endpoint, method, params);
      if (value !== undefined) return value;
    }
    if (method === "session.snapshot")
      return { snapshot: { workspaces: [...workspaces], panes: [...panes] } };
    if (method === "workspace.create") {
      const workspace = {
        workspace_id: `w${workspaces.length + 1}`,
        label: params.label,
      };
      const pane = {
        pane_id: `p${panes.length + 1}`,
        workspace_id: workspace.workspace_id,
        cwd: params.cwd,
      };
      workspaces.push(workspace);
      panes.push(pane);
      return { workspace, root_pane: pane };
    }
    if (method === "pane.split") {
      assert.ok(
        workspaces.some(
          (workspace) => workspace.workspace_id === params.workspace_id,
        ),
      );
      const pane = {
        pane_id: `p${panes.length + 1}`,
        workspace_id: params.workspace_id,
        cwd: params.cwd,
      };
      panes.push(pane);
      return { pane };
    }
    if (method === "pane.send_input") return {};
    throw new Error(`Unexpected Herdr method ${method}`);
  };
  const launcher = new SessionLauncher({
    getConnections: () => connections,
    rpc,
    modelProviders: options.modelProviders,
    checkCompatibility: options.checkCompatibility,
    readSnapshot: options.readSnapshot,
    worktreeCreate: options.worktreeCreate,
    userDataDir: options.userDataDir,
    savedWorkspaces: options.savedWorkspaces,
    resolveEnvironment: options.resolveEnvironment,
    prepareSession: options.prepareSession,
  });
  return { launcher, calls, workspaces, panes, connections, rpc };
}

test("20 independent simultaneous project starts produce one workspace and 20 panels", async () => {
  const { launcher, workspaces, panes, calls } = setup();
  const results = await Promise.all(
    Array.from({ length: 20 }, (_, index) =>
      launcher.launch(launch(`intent-${index}`)),
    ),
  );
  assert.ok(results.every((result) => result.ok));
  assert.equal(workspaces.length, 1);
  assert.equal(panes.length, 20);
  assert.equal(
    calls.filter((call) => call.method === "session.snapshot").length,
    1,
  );
  assert.equal(new Set(results.map((result) => result.value.paneId)).size, 20);
});

test("saved canonical checkout survives shell cd and launcher restart", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-canonical-journal-"),
  );
  try {
    const server = setup({ userDataDir: directory });
    const first = await server.launcher.launch(launch("original"));
    assert.equal(first.ok, true);
    server.panes[0].cwd = "/tmp/different-project";
    const restarted = new SessionLauncher({
      getConnections: () => server.connections,
      rpc: server.rpc,
      userDataDir: directory,
    });
    const next = await restarted.launch(launch("separate-panel"));
    assert.equal(next.ok, true);
    assert.equal(next.value.workspaceId, first.value.workspaceId);
    assert.equal(server.workspaces.length, 1);
    assert.equal(server.panes.length, 2);
  } finally {
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("migrated saved checkout reuses legacy sessions after cd without a launch journal", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-saved-checkout-"),
  );
  try {
    const server = setup({
      savedWorkspaces: () => savedWorkspaces(directory),
    });
    await server.rpc("/tmp/launch-herdr.sock", "workspace.create", {
      cwd: "/tmp/checkout",
    });
    server.panes[0].cwd = "/tmp/elsewhere";
    writeSnapshotSync(
      directory,
      JSON.stringify({
        workspaces: [
          {
            id: "ws-1",
            connection: "/tmp/launch-herdr.sock",
            herdrId: "w1",
            cwd: "/tmp/alias",
          },
        ],
      }),
    );
    const next = await server.launcher.launch(launch("new-panel"));
    assert.equal(next.ok, true);
    assert.equal(next.value.workspaceId, "w1");
    assert.equal(server.workspaces.length, 1);
    assert.equal(server.panes.length, 2);
  } finally {
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("project association remains after its original pane closes", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-journal-project-"),
  );
  try {
    const server = setup({ userDataDir: directory });
    const first = await server.launcher.launch(launch("original"));
    assert.equal(first.ok, true);
    server.panes.splice(0, 1, {
      pane_id: "external",
      workspace_id: "w1",
      cwd: "/tmp/elsewhere",
    });
    const restarted = new SessionLauncher({
      getConnections: () => server.connections,
      rpc: server.rpc,
      userDataDir: directory,
    });
    const next = await restarted.launch(launch("new-panel"));
    assert.equal(next.ok, true);
    assert.equal(server.workspaces.length, 1);
    assert.equal(server.panes.length, 2);
  } finally {
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("same request shares its pending result and replay does not create another pane", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const { launcher, workspaces, panes } = setup({
    checkCompatibility: () => gate,
  });
  const input = launch("same-intent");
  const first = launcher.launch(input);
  const repeat = launcher.launch({ ...input });
  assert.equal(first, repeat);
  assert.equal(launcher.queues.size, 1);
  release();
  const result = await first;
  assert.equal(result.ok, true);
  assert.deepEqual(await launcher.launch(input), result);
  assert.equal(workspaces.length, 1);
  assert.equal(panes.length, 1);
  const conflict = await launcher.launch({
    ...input,
    cwd: "/tmp/another-checkout",
  });
  assert.equal(conflict.error.code, "OPERATION_CONFLICT");
});

test("canonical lookup preserves existing live sessions and keeps checkouts/worktrees separate", async () => {
  const { launcher, workspaces, panes } = setup();
  workspaces.push(
    { workspace_id: "intentional-a" },
    { workspace_id: "intentional-b" },
  );
  panes.push(
    { pane_id: "one", workspace_id: "intentional-a", cwd: "/tmp/checkout/" },
    { pane_id: "two", workspace_id: "intentional-b", cwd: "/tmp/alias" },
  );
  const aliased = await launcher.launch(launch("alias", { cwd: "/tmp/alias" }));
  assert.equal(aliased.value.workspaceId, "intentional-a");
  assert.equal(workspaces.length, 2);
  assert.equal(panes.length, 3);
  await launcher.launch(
    launch("worktree", { cwd: "/tmp/checkout-other-tree" }),
  );
  await launcher.launch(launch("checkout", { cwd: "/tmp/another-checkout" }));
  assert.equal(workspaces.length, 4);
  assert.ok(panes.some((pane) => pane.pane_id === "one"));
  assert.ok(panes.some((pane) => pane.pane_id === "two"));
});

test("endpoint queues keep identical daemon IDs in separate projects", async () => {
  const { launcher, calls } = setup({
    rpc: async (_, method) =>
      method === "workspace.create"
        ? { workspace: { workspace_id: "w1" }, root_pane: { pane_id: "p1" } }
        : undefined,
  });
  const results = await Promise.all([
    launcher.launch(launch("op", { endpoint: "/tmp/one.sock" })),
    launcher.launch(launch("op", { endpoint: "/tmp/two.sock" })),
  ]);
  assert.ok(results.every((result) => result.ok));
  assert.equal(
    calls.filter((call) => call.method === "workspace.create").length,
    2,
  );
});

test("agent preparation failure records creation and a retry uses that pane", async () => {
  let failures = 1;
  const { launcher, workspaces, panes } = setup({
    modelProviders: {
      resolveEnv: async () => ({
        settings: { ANTHROPIC_MODEL: "configured" },
        key: "synthetic-test-key",
      }),
    },
    exec: async () => {
      if (failures--)
        throw Object.assign(new Error("Settings transfer failed"), {
          code: "TRANSFER_FAILED",
        });
      return "/tmp/remote-session-env";
    },
  });
  const input = launch("retry-agent", {
    endpoint: "ssh:example-host",
    kind: "agent",
    agent: "claude",
    modelProfileId: "profile",
  });
  const failed = await launcher.launch(input);
  assert.equal(failed.ok, false);
  assert.equal(failed.error.stage, "agent");
  assert.equal(failed.error.created.paneId, "p1");
  const retried = await launcher.launch(input);
  assert.equal(retried.ok, true);
  assert.equal(retried.value.paneId, "p1");
  assert.equal(workspaces.length, 1);
  assert.equal(panes.length, 1);
});

test("unconfigured environment is distinct from a failed configured environment read", async () => {
  const { launcher, workspaces } = setup({
    modelProviders: {
      resolveEnv: async () => {
        throw Object.assign(new Error("Profile read failed"), {
          code: "EACCES",
        });
      },
    },
  });
  const failed = await launcher.launch(
    launch("configured", { modelProfileId: "profile" }),
  );
  assert.equal(failed.error.code, "EACCES");
  assert.equal(failed.error.stage, "environment");
  assert.equal(workspaces.length, 0);
  assert.equal((await launcher.launch(launch("unconfigured"))).ok, true);
  assert.equal(workspaces.length, 1);
});

test("creation, split, restore and SSH worktree share env/locale preparation", async () => {
  const { launcher, calls, panes, workspaces } = setup({
    modelProviders: {
      resolveEnv: async () => ({
        settings: { ANTHROPIC_MODEL: "configured" },
        key: "synthetic-test-key",
      }),
    },
  });
  const env = { SUSHIAI_PROBE: "Unicode 🍣 漢字", LANG: "C" };
  await launcher.launch(launch("shell", { env }));
  await launcher.launch(
    launch("split", { env, workspaceId: "w1", targetPaneId: "p1" }),
  );
  await launcher.launch(
    launch("restore", {
      env,
      workspaceId: "w1",
      targetPaneId: "p1",
      restore: true,
    }),
  );
  await launcher.launch(
    launch("remote-agent", {
      endpoint: "ssh:example-host",
      env,
      kind: "agent",
      agent: "claude",
      modelProfileId: "profile",
      worktree: { branch: "probe" },
    }),
  );
  assert.equal(workspaces.length, 2);
  assert.equal(panes.length, 4);
  const mutations = calls.filter((call) =>
    ["workspace.create", "pane.split"].includes(call.method),
  );
  assert.equal(mutations.length, 4);
  for (const { params } of mutations) {
    assert.equal(params.env.SUSHIAI_PROBE, env.SUSHIAI_PROBE);
    assert.match(params.env.LANG, /UTF-8/);
    assert.equal(params.env.LC_ALL, params.env.LC_CTYPE);
  }
  assert.equal(mutations.at(-1).params.cwd, "/tmp/checkout-branch");
  assert.equal(mutations.at(-1).params.env.ANTHROPIC_MODEL, "configured");
  const agentInput = calls.find((call) => call.method === "pane.send_input");
  assert.match(
    agentInput.params.text,
    /^\(\. '\/tmp\/remote-session-env' \|\| exit; .*exec claude --settings /,
  );
  assert.ok(!agentInput.params.text.includes("synthetic-test-key"));
  assert.ok(
    calls.some(
      (call) =>
        call.payload?.includes("synthetic-test-key") &&
        call.endpoint === "ssh:example-host",
    ),
  );
  assert.ok(!calls.some((call) => call.method === "worktree.create"));
});

test("local and SSH worktrees use the prepared primary checkout and chosen base", async () => {
  for (const endpoint of ["/tmp/base-herdr.sock", "ssh:example-host"])
    for (const base of [undefined, "refs/remotes/origin/release/stable"]) {
      const creations = [];
      const state = setup({
        inspect: async (_, input) => {
          if (input.operation === "canonical_checkout")
            return { cwd: input.root };
          if (input.operation === "worktree_base") {
            assert.equal(input.root, "/tmp/linked-feature");
            assert.equal(input.base, base);
            return { cwd: "/tmp/primary-checkout", base: "fresh-commit" };
          }
          if (input.operation === "worktree_create") {
            creations.push([input.root, input.branch, input.base]);
            return { path: "/tmp/primary-checkout-probe" };
          }
          throw new Error(`Unexpected inspection ${input.operation}`);
        },
        worktreeCreate: async (...args) => {
          creations.push(args);
          return { path: "/tmp/primary-checkout-probe" };
        },
      });
      const result = await state.launcher.launch(
        launch("base", {
          endpoint,
          cwd: "/tmp/linked-feature",
          worktree: { branch: "probe", base },
        }),
      );
      assert.equal(result.ok, true);
      assert.deepEqual(creations, [
        ["/tmp/primary-checkout", "probe", "fresh-commit"],
      ]);
      assert.equal(result.value.cwd, "/tmp/primary-checkout-probe");
      assert.equal(state.workspaces.length, 1);
    }
});

test("a failed worktree base preparation creates no checkout or session and remains retryable", async () => {
  let fail = true;
  let creations = 0;
  const state = setup({
    inspect: async (_, input) => {
      if (input.operation === "canonical_checkout") return { cwd: input.root };
      if (input.operation === "worktree_base") {
        if (fail) throw new Error("Base fetch failed");
        return { cwd: "/tmp/checkout", base: "fresh-commit" };
      }
      throw new Error(`Unexpected inspection ${input.operation}`);
    },
    worktreeCreate: async () => {
      creations += 1;
      return { path: "/tmp/checkout-probe" };
    },
  });
  const input = launch("base-retry", { worktree: { branch: "probe" } });
  const failed = await state.launcher.launch(input);
  assert.equal(failed.ok, false);
  assert.equal(failed.error.stage, "checkout");
  assert.equal(failed.error.retryable, true);
  assert.equal(creations, 0);
  assert.equal(state.workspaces.length, 0);
  fail = false;
  assert.equal((await state.launcher.launch(input)).ok, true);
  assert.equal(creations, 1);
  assert.equal(state.workspaces.length, 1);
});

test("restoration reacts to structured missing codes and never to message text", async () => {
  let missing = true;
  const { launcher, workspaces } = setup({
    rpc: async (_, method) => {
      if (method === "pane.split" && missing) {
        missing = false;
        throw Object.assign(new Error("Arbitrary localized wording"), {
          code: "workspace_not_found",
        });
      }
    },
  });
  const restored = await launcher.launch(
    launch("restore", {
      workspaceId: "gone",
      targetPaneId: "gone-pane",
      restore: true,
    }),
  );
  assert.equal(restored.ok, true);
  assert.equal(restored.value.createdWorkspace, true);
  assert.equal(workspaces.length, 1);
  const textOnly = setup({
    rpc: async (_, method) => {
      if (method === "pane.split") throw new Error("workspace_not_found");
    },
  });
  const failure = await textOnly.launcher.launch(
    launch("text-error", { workspaceId: "gone", restore: true }),
  );
  assert.equal(failure.ok, false);
  assert.equal(textOnly.workspaces.length, 0);
});

test("new service instance resumes a recorded created pane without creating another", async () => {
  const state = setup();
  const first = await state.launcher.launch(launch("persisted"));
  const recreated = new SessionLauncher({
    getConnections: () => state.connections,
    rpc: state.rpc,
  });
  const resumed = await recreated.launch(
    launch("persisted", {
      workspaceId: first.value.workspaceId,
      paneId: first.value.paneId,
    }),
  );
  assert.equal(resumed.ok, true);
  assert.equal(state.workspaces.length, 1);
  assert.equal(state.panes.length, 1);
});

test("registered IPC launch entry returns structured errors and shares its operation queue", async () => {
  const state = setup();
  const handlers = new Map();
  registerSessionLaunchIpc({
    handle: (channel, handler) => handlers.set(channel, handler),
    getConnections: () => state.connections,
    rpc: state.rpc,
  });
  const handler = handlers.get("session-launch");
  const repeated = await Promise.all([
    handler(launch("ipc")),
    handler(launch("ipc")),
  ]);
  assert.deepEqual(repeated[0], repeated[1]);
  assert.equal(state.panes.length, 1);
  const invalid = await handler(
    launch("invalid", { env: { "BAD KEY": "value" } }),
  );
  assert.equal(invalid.ok, false);
  assert.equal(invalid.error.retryable, false);
  assert.equal(invalid.error.stage, "validation");
});

test("creation journal survives application restart without persisting environment or keys", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-launch-journal-"),
  );
  let preparationFailed = false;
  try {
    const modelProviders = {
      resolveEnv: async () => ({
        settings: { ANTHROPIC_MODEL: "configured" },
        key: "synthetic-key-never-in-journal",
      }),
    };
    const state = setup({
      userDataDir: directory,
      modelProviders,
      exec: async () => {
        const saved = JSON.parse(
          appDb(directory).prepare("SELECT data FROM launches").get().data,
        );
        assert.equal(saved.created.paneId, "p1");
        if (!preparationFailed) {
          preparationFailed = true;
          throw new Error("Interrupted after creation was persisted");
        }
        return "/tmp/remote-session-env";
      },
    });
    const input = launch("durable", {
      endpoint: "ssh:example-host",
      kind: "agent",
      agent: "claude",
      modelProfileId: "profile",
      env: { PRIVATE_PROBE: "synthetic-env-not-for-journal" },
    });
    const failure = await state.launcher.launch(input);
    assert.equal(failure.ok, false);
    assert.equal(failure.error.created.paneId, "p1");
    const text = appDb(directory)
      .prepare("SELECT data FROM launches")
      .all()
      .map((row) => row.data)
      .join("\n");
    assert.ok(!text.includes("synthetic-key-never-in-journal"));
    assert.ok(!text.includes("synthetic-env-not-for-journal"));
    const options = {
      userDataDir: directory,
      modelProviders,
      getConnections: () => state.connections,
      rpc: state.rpc,
    };
    const restarted = new SessionLauncher(options);
    const resumed = await restarted.launch(input);
    assert.equal(resumed.ok, true);
    assert.equal(resumed.value.paneId, "p1");
    const completedRestart = new SessionLauncher(options);
    assert.equal((await completedRestart.launch(input)).ok, true);
    assert.equal(state.panes.length, 1);
    assert.equal(
      state.calls.filter((call) => call.method === "pane.send_input").length,
      1,
    );
  } finally {
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("canonical checkout is resolved on the host, separating symlinks, worktrees and checkouts", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const { execFile } = require("node:child_process");
  const { promisify } = require("node:util");
  const { Connections } = require("../electron/connections.cjs");
  const execute = promisify(execFile);
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-canonical-checkout-"),
  );
  const connections = new Connections(path.join(directory, "application"));
  try {
    const root = path.join(directory, "repo"),
      subdir = path.join(root, "src"),
      alias = path.join(directory, "alias"),
      checkout = path.join(directory, "other-checkout"),
      worktree = path.join(directory, "repo-other-tree");
    await fs.mkdir(subdir, { recursive: true });
    await fs.mkdir(checkout);
    await fs.symlink(root, alias);
    await execute("git", ["init", root]);
    await execute("git", [
      "-C",
      root,
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.com",
      "commit",
      "--allow-empty",
      "-m",
      "fixture",
    ]);
    await execute("git", [
      "-C",
      root,
      "worktree",
      "add",
      "-b",
      "other-tree",
      worktree,
    ]);
    await execute("git", ["init", checkout]);
    const canonical = async (selected) =>
      (
        await connections.inspect("/tmp/test.sock", {
          operation: "canonical_checkout",
          root: selected,
        })
      ).cwd;
    const canonicalRoot = await fs.realpath(root);
    assert.equal(await canonical(alias), canonicalRoot);
    assert.equal(await canonical(subdir), canonicalRoot);
    assert.equal(await canonical(`${root}/`), canonicalRoot);
    assert.equal(await canonical(worktree), await fs.realpath(worktree));
    assert.equal(await canonical(checkout), await fs.realpath(checkout));
  } finally {
    await connections.close();
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("the host worktree helper defaults to main and accepts the prepared commit", async () => {
  const fs = require("node:fs/promises");
  const os = require("node:os");
  const path = require("node:path");
  const { execFile } = require("node:child_process");
  const { promisify } = require("node:util");
  const { Connections } = require("../electron/connections.cjs");
  const execute = promisify(execFile);
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-host-worktree-base-"),
  );
  const connections = new Connections(path.join(directory, "application"));
  const root = path.join(directory, "repo");
  const git = async (...args) =>
    (await execute("git", ["-C", root, ...args])).stdout.trim();
  const commit = () =>
    git(
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.test",
      "commit",
      "--allow-empty",
      "-m",
      "fixture",
    );
  try {
    await execute("git", ["init", "-b", "main", root]);
    await commit();
    const main = await git("rev-parse", "HEAD");
    await git("checkout", "-b", "feature");
    await commit();
    const feature = await git("rev-parse", "HEAD");
    for (const [branch, base, expected] of [
      ["probe-default", undefined, main],
      ["probe-selected", feature, feature],
    ]) {
      const input = { operation: "worktree_create", root, branch };
      if (base) input.base = base;
      const created = await connections.inspect("/tmp/test.sock", input);
      const head = await execute("git", [
        "-C",
        created.path,
        "rev-parse",
        "HEAD",
      ]);
      assert.equal(head.stdout.trim(), expected);
    }
  } finally {
    await connections.close();
    closeAppDb(directory);
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("upstream project values reach native env for create, split, restore and worktree without being typed", async (t) => {
  const { makeStore } = require("./helpers/fake-host.cjs");
  const { sessionEnvironment } = require("../electron/project-session.cjs");
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "Checkout",
    git: { url: "git@example.test:acme/checkout.git", defaultBranch: "main" },
    env: [{ name: "PROJECT_TOKEN", secret: true }],
  });
  await projects.setSecret(
    project.id,
    "PROJECT_TOKEN",
    "synthetic-native-secret",
  );
  for (const endpoint of ["local", "ssh:example-host"])
    await projects.attach({
      remote: project.git.url,
      endpoint,
      cwd: "/tmp/checkout",
      name: "Checkout",
    });
  let connections;
  const state = setup({
    resolveEnvironment: (input) =>
      sessionEnvironment({ projects, connections }, input),
  });
  connections = state.connections;
  for (const [operationId, extra] of [
    ["create", {}],
    ["split", { workspaceId: "w1", targetPaneId: "p1" }],
    ["restore", { workspaceId: "w1", targetPaneId: "p1", restore: true }],
    [
      "worktree",
      { endpoint: "ssh:example-host", worktree: { branch: "probe" } },
    ],
  ])
    assert.equal(
      (await state.launcher.launch(launch(operationId, extra))).ok,
      true,
    );
  for (const call of state.calls.filter((call) =>
    ["workspace.create", "pane.split"].includes(call.method),
  ))
    assert.equal(call.params.env.PROJECT_TOKEN, "synthetic-native-secret");
  assert.equal(
    state.calls.filter(
      (call) => call.method === "pane.send_input" || call.payload,
    ).length,
    0,
  );
  await projects.setHostWithheld(project.id, "ssh:example-host", true);
  const withheld = await state.launcher.launch(
    launch("withheld", { endpoint: "ssh:example-host" }),
  );
  assert.equal(withheld.ok, true);
  assert.equal(
    state.calls
      .filter((call) =>
        ["workspace.create", "pane.split"].includes(call.method),
      )
      .at(-1).params.env.PROJECT_TOKEN,
    undefined,
  );
});

test("project environment read failure stops creation and remains retryable", async () => {
  let fail = true;
  const { sessionEnvironment } = require("../electron/project-session.cjs");
  const state = setup({
    resolveEnvironment: (input) =>
      sessionEnvironment(
        {
          projects: {
            resolveProject: async () => ({ id: "project" }),
            sendsValues: async () => true,
            environmentFor: async () => {
              if (fail)
                throw Object.assign(new Error("Project store read failed"), {
                  code: "EACCES",
                });
              return { RECOVERED: "yes" };
            },
          },
        },
        input,
      ),
  });
  const input = launch("read-failed");
  const failed = await state.launcher.launch(input);
  assert.equal(failed.error.code, "EACCES");
  assert.equal(failed.error.stage, "environment");
  assert.equal(failed.error.retryable, true);
  assert.equal(state.panes.length, 0);
  fail = false;
  assert.equal((await state.launcher.launch(input)).ok, true);
  assert.equal(state.panes.length, 1);
});

test("native account preparation distinguishes missing project account from resolver failure", async () => {
  const { sessionEnvPrefix } = require("../electron/project-session.cjs");
  const nativeEnvironment = {
    project: { sessions: { claudeAccount: "own" } },
    sends: true,
    env: { PROJECT_TOKEN: "not-in-prefix" },
  };
  const input = {
    endpoint: "/tmp/account.sock",
    cwd: "/tmp/checkout",
    kind: "agent",
    agent: "claude",
    nativeEnvironment,
  };
  const missing = async () => {
    throw Object.assign(new Error("Not configured"), {
      code: "ACCOUNT_NOT_CONFIGURED",
    });
  };
  assert.deepEqual(await sessionEnvPrefix({ resolveAccount: missing }, input), {
    prefix: "",
    settings: "",
    launch: "",
  });
  await assert.rejects(
    sessionEnvPrefix(
      { resolveAccount: missing },
      { ...input, claudeAccountId: "picked" },
    ),
    { code: "ACCOUNT_NOT_CONFIGURED" },
  );
  await assert.rejects(
    sessionEnvPrefix(
      {
        resolveAccount: async () => {
          throw new SyntaxError("Malformed account store");
        },
      },
      input,
    ),
    SyntaxError,
  );
  const codexInput = {
    ...input,
    agent: "codex",
    nativeEnvironment: {
      ...nativeEnvironment,
      project: { sessions: { codexAccount: "own" } },
    },
  };
  assert.deepEqual(
    await sessionEnvPrefix({ resolveCodexAccount: missing }, codexInput),
    { prefix: "", settings: "", launch: "" },
  );
  await assert.rejects(
    sessionEnvPrefix(
      {
        resolveCodexAccount: async () => {
          throw Object.assign(new Error("Read failed"), { code: "EACCES" });
        },
      },
      codexInput,
    ),
    { code: "EACCES" },
  );
});

test("native SSH account preparation uses private temporary files and rejects failed upload", async () => {
  const { sessionEnvPrefix } = require("../electron/project-session.cjs");
  const nativeEnvironment = {
    project: null,
    sends: false,
    env: { PROJECT_TOKEN: "native-only" },
  };
  const input = {
    endpoint: "ssh:example-host",
    cwd: "/tmp/checkout",
    agent: "claude",
    claudeAccountId: "picked",
    nativeEnvironment,
  };
  let payload;
  const options = {
    resolveAccount: async () => ({
      kind: "apiKey",
      value: "synthetic-account-key",
    }),
    upload: async (_, text) => {
      payload = text;
      return "/tmp/account-env";
    },
    remove: async () => {},
  };
  const prepared = await sessionEnvPrefix(options, input);
  assert.ok(payload.includes("synthetic-account-key"));
  assert.ok(!payload.includes("native-only"));
  assert.ok(!prepared.prefix.includes("synthetic-account-key"));
  assert.match(prepared.prefix, /\|\| exit/);
  assert.match(prepared.settings, /apiKeyHelper/);
  await assert.rejects(
    sessionEnvPrefix(
      {
        ...options,
        upload: async () => {
          throw new Error("Upload failed");
        },
      },
      input,
    ),
    /Upload failed/,
  );
  await assert.rejects(
    sessionEnvPrefix({ ...options, upload: async () => "" }, input),
    /did not confirm/,
  );
});
