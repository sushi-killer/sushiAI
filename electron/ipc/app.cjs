const os = require("node:os");
const path = require("node:path");
const { powerSaveBlocker } = require("electron");

function registerAppIpc({
  handle,
  app,
  dialog,
  shell,
  getMainWindow,
  getConnections,
  getUpdates,
  agents,
  claudeMcp,
  claudePlugins,
  modelProviders,
  executable,
  scanLocalSkills,
  manageSkill,
  userDataDir,
  stageModelSettings,
}) {
  let skillsCatalogCache;
  let skillsCatalogScan;

  const connections = () => {
    const value = getConnections();
    if (!value) throw new Error("Connections are not ready.");
    return value;
  };

  const updates = () => {
    const value = getUpdates();
    if (!value) throw new Error("Updates are not ready.");
    return value;
  };

  handle("agent-providers", () => agents.list());
  handle("agent-call", (provider, operation, input) =>
    agents.call(provider, operation, input),
  );
  handle("agent-open-external", async (value) => {
    if (typeof value !== "string" || value.length > 8192)
      throw new Error("Invalid link.");
    const url = new URL(value);
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password
    )
      throw new Error("Only web links can be opened.");
    await shell.openExternal(url.href);
  });

  handle("updates-state", () => updates().snapshot());
  handle("updates-check", () => updates().check());
  handle("updates-download", () => updates().download());
  handle("updates-configure", (settings) => updates().configure(settings));
  handle("updates-open", () => updates().openInstaller());
  handle("updates-install", () => updates().install());
  handle("updates-release-page", () => updates().releasePage());

  handle("system", async () => ({
    home: os.homedir(),
    cwd: app.isPackaged ? os.homedir() : process.cwd(),
    platform: process.platform,
    socketPath:
      process.env.HERDR_SOCKET_PATH ||
      path.join(os.homedir(), ".config/herdr/herdr.sock"),
    agents: ["claude", "codex", "gemini", "cursor-agent", "herdr"].map(
      (name) => ({ name, path: executable(name) }),
    ),
  }));

  // Idle sleep kills a long agent turn mid-flight, which is why people end up
  // babysitting `caffeinate` in a terminal. Electron's own blocker does the
  // same job without a child process: `prevent-app-suspension` stops the
  // system sleeping while still letting the display go dark, which is the
  // behaviour `caffeinate -i` gives and what anyone leaving an agent running
  // overnight actually wants.
  let awakeBlocker = null;
  handle("keep-awake", async (on) => {
    const active =
      awakeBlocker !== null && powerSaveBlocker.isStarted(awakeBlocker);
    if (on && !active)
      awakeBlocker = powerSaveBlocker.start("prevent-app-suspension");
    else if (!on && active) {
      powerSaveBlocker.stop(awakeBlocker);
      awakeBlocker = null;
    }
    return awakeBlocker !== null && powerSaveBlocker.isStarted(awakeBlocker);
  });

  handle("choose-attachments", async () => {
    const result = await dialog.showOpenDialog(getMainWindow(), {
      properties: ["openFile", "openDirectory", "multiSelections"],
    });
    return result.canceled ? [] : result.filePaths;
  });
  handle("choose-directory", async () => {
    const result = await dialog.showOpenDialog(getMainWindow(), {
      properties: ["openDirectory", "createDirectory"],
    });
    return result.canceled ? null : result.filePaths[0];
  });
  handle("window", (action) => {
    const mainWindow = getMainWindow();
    if (action === "minimize") mainWindow.minimize();
    if (action === "maximize")
      mainWindow.setFullScreen(!mainWindow.isFullScreen());
    if (action === "close") mainWindow.close();
  });

  handle("catalog", async (kind, options = {}) => {
    if (kind !== "skills") return [];
    const snapshotFile = path.join(userDataDir(), "skills-catalog.json");
    const force = Boolean(options?.force);
    if (!force && skillsCatalogCache?.snapshotFile === snapshotFile)
      return skillsCatalogCache.items;
    if (skillsCatalogScan) return skillsCatalogScan;
    skillsCatalogScan = scanLocalSkills({ snapshotFile })
      .then((items) => {
        skillsCatalogCache = { snapshotFile, items };
        return items;
      })
      .finally(() => {
        skillsCatalogScan = undefined;
      });
    return skillsCatalogScan;
  });
  handle("skills-manage", async (action, item) => {
    const result = await manageSkill({
      home: os.homedir(),
      action,
      item,
      shell,
      executable,
    });
    skillsCatalogCache = undefined;
    return result;
  });

  handle("claude-mcp-list", (cwd, endpoint) =>
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? connections().inspect(endpoint, {
          operation: "claude_mcp",
          action: "list",
          cwd,
        })
      : claudeMcp.list(cwd),
  );
  handle("claude-mcp-toggle", (input) => {
    if (!input || typeof input !== "object" || Array.isArray(input))
      throw new Error("Invalid MCP request.");
    const { endpoint, ...request } = input;
    return typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? connections().inspect(endpoint, {
          operation: "claude_mcp",
          action: "toggle",
          ...request,
        })
      : claudeMcp.toggle(request);
  });
  handle("claude-plugins-list", (cwd, endpoint) =>
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? connections().inspect(endpoint, {
          operation: "claude_plugins",
          action: "list",
          cwd,
        })
      : claudePlugins.list(cwd),
  );
  handle("claude-plugins-toggle", (input) => {
    if (!input || typeof input !== "object" || Array.isArray(input))
      throw new Error("Invalid plugin request.");
    const { endpoint, ...request } = input;
    return typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? connections().inspect(endpoint, {
          operation: "claude_plugins",
          action: "toggle",
          ...request,
        })
      : claudePlugins.toggle(request);
  });

  handle("providers-list", () => modelProviders.listProviders());
  handle("providers-upsert", (input) => modelProviders.upsertProvider(input));
  handle("providers-delete", (providerId) =>
    modelProviders.deleteProvider(providerId),
  );
  handle("providers-set-key", (providerId, key) =>
    modelProviders.setProviderKey(providerId, key),
  );
  handle("providers-clear-key", (providerId) =>
    modelProviders.clearProviderKey(providerId),
  );
  handle("providers-test", (providerId) =>
    modelProviders.testConnection(providerId),
  );
  handle("providers-models", (providerId) =>
    modelProviders.fetchModels(providerId),
  );
  handle("model-profiles-list", () => modelProviders.listProfiles());
  handle("model-profiles-upsert", (input) =>
    modelProviders.upsertProfile(input),
  );
  handle("model-profiles-delete", (profileId) =>
    modelProviders.deleteProfile(profileId),
  );
  handle("model-settings-stage", (modelProfileId) =>
    stageModelSettings(modelProfileId),
  );
}

module.exports = { registerAppIpc };
