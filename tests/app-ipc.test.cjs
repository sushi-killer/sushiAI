const { test } = require("node:test");
const assert = require("node:assert/strict");

const { registerAppIpc } = require("../electron/ipc/app.cjs");

function setup() {
  const handlers = new Map();
  const calls = [];
  const mainWindow = {
    minimize: () => calls.push("minimize"),
    close: () => calls.push("close"),
    isFullScreen: () => false,
    setFullScreen: (value) => calls.push(["fullscreen", value]),
  };
  const modelProviders = {
    listProviders: () => ["provider"],
    upsertProvider: (input) => ({ input }),
    deleteProvider: (id) => calls.push(["delete-provider", id]),
    setProviderKey: (id, key) => ({ id, key }),
    clearProviderKey: (id) => calls.push(["clear-key", id]),
    testConnection: (id) => ({ id, ok: true }),
    fetchModels: (id) => [{ id }],
    listProfiles: () => ["profile"],
    upsertProfile: (input) => ({ input }),
    deleteProfile: (id) => calls.push(["delete-profile", id]),
  };
  registerAppIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    app: { isPackaged: false },
    dialog: {
      showOpenDialog: async () => ({
        canceled: false,
        filePaths: ["/tmp/file"],
      }),
    },
    shell: { openExternal: async (url) => calls.push(["external", url]) },
    getMainWindow: () => mainWindow,
    getConnections: () => ({
      inspect: async (endpoint, input) => ({ endpoint, input }),
    }),
    getUpdates: () => ({
      snapshot: () => ({ phase: "idle" }),
      check: () => "check",
      download: () => "download",
      configure: (input) => input,
      openInstaller: () => "open",
      install: () => "install",
      releasePage: () => "release",
    }),
    agents: {
      list: () => ["agent"],
      call: (provider, operation, input) => ({ provider, operation, input }),
    },
    claudeMcp: { list: (cwd) => ({ cwd }), toggle: (input) => input },
    claudePlugins: { list: (cwd) => ({ cwd }), toggle: (input) => input },
    modelProviders,
    executable: (name) => `/bin/${name}`,
    scanLocalSkills: async () => [{ name: "skill" }],
    manageSkill: async (action) => ({ action }),
    userDataDir: () => "/tmp/sushiai-app-ipc",
    stageModelSettings: async (id) => `/tmp/${id}.json`,
  });
  return { handlers, calls };
}

test("app IPC registration keeps core channels outside main and preserves contracts", async () => {
  const { handlers, calls } = setup();
  for (const channel of [
    "agent-providers",
    "updates-state",
    "system",
    "catalog",
    "claude-mcp-list",
    "claude-plugins-list",
    "providers-list",
    "model-profiles-list",
    "model-settings-stage",
  ])
    assert.equal(handlers.has(channel), true, channel);
  assert.deepEqual(await handlers.get("agent-providers")(), ["agent"]);
  assert.deepEqual(await handlers.get("updates-state")(), { phase: "idle" });
  assert.equal((await handlers.get("system")()).cwd, process.cwd());
  assert.deepEqual(await handlers.get("catalog")("skills"), [
    { name: "skill" },
  ]);
  assert.deepEqual(await handlers.get("providers-list")(), ["provider"]);
  assert.equal(
    await handlers.get("model-settings-stage")("profile"),
    "/tmp/profile.json",
  );
  await handlers.get("window")("minimize");
  await handlers.get("agent-open-external")("https://example.com");
  assert.deepEqual(calls, ["minimize", ["external", "https://example.com/"]]);
});

test("app IPC rejects unsafe external links before handing them to the shell", async () => {
  const { handlers, calls } = setup();
  await assert.rejects(
    () => handlers.get("agent-open-external")("file:///tmp/a"),
    /Only web links/,
  );
  await assert.rejects(
    () => handlers.get("agent-open-external")("https://user:pass@example.com"),
    /Only web links/,
  );
  assert.deepEqual(calls, []);
});
