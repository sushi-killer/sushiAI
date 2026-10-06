const {
  app,
  BrowserWindow,
  ipcMain,
  screen,
  dialog,
  Menu,
  session,
  shell,
  safeStorage,
  powerMonitor,
  globalShortcut,
} = require("electron");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs/promises");
const { existsSync } = require("node:fs");
const pty = require("node-pty");
const { Connections } = require("./connections.cjs");
const { PreviewServer } = require("./preview.cjs");
const { Updates } = require("./updates.cjs");
const { scanLocalSkills } = require("./skills-catalog.cjs");
const { manageSkill } = require("./skills-manager.cjs");
const { ClaudeMcp } = require("./claude-mcp.cjs");
const { ClaudePlugins } = require("./claude-plugins.cjs");
const { ModelProviders } = require("./model-providers.cjs");
const { CodexAccounts } = require("./codex-accounts.cjs");
const { Projects } = require("./projects.cjs");
const { AgentRegistry } = require("./agents/registry.cjs");
const { HermesProvider } = require("./agents/hermes-provider.cjs");
const { registerProjectIpc } = require("./ipc/projects.cjs");
const { registerTerminalIpc } = require("./ipc/terminals.cjs");
const {
  setupHost,
  setupSummary,
  requireHostManifest,
  setHostManifestSource,
} = require("./host-setup.cjs");
const { registerChatIpc } = require("./ipc/chat.cjs");
const { registerAppIpc } = require("./ipc/app.cjs");
const { ipcResult } = require("./ipc/errors.cjs");
const { createDaemonLaunch } = require("./session-launch.cjs");
const { registerExtensionIpc } = require("./ipc/extensions.cjs");
const { registerDaemonIpc, forwardDaemonEvents } = require("./ipc/daemon.cjs");
const { createDaemonManager } = require("./daemon/manager.cjs");
const { createLocalConnector } = require("./daemon/local.cjs");
const { remoteConnectors } = require("./daemon/connectors.cjs");
const { createHostInstaller } = require("./daemon/install.cjs");
const { registerAttentionIpc } = require("./attention.cjs");
const { registerWorkspaceSnapshot } = require("./workspace-snapshot.cjs");
const {
  DEFAULT_BOUNDS,
  loadWindowState,
  saveWindowState,
  clampBounds,
} = require("./window-state.cjs");
const { registerMascot, watchPresenting } = require("./mascot.cjs");
const { createMascotShortcut } = require("./mascot-shortcut.cjs");
const { DEV_RESTART_EXIT_CODE, watchCore } = require("./dev-restart.cjs");
const { testWindow } = require("./test-window.cjs");

const testMode = testWindow();
const { SurfaceStateStore } = require("./extensions/surface-state.cjs");
const { ExtensionManager } = require("./extensions/extension-manager.cjs");
const {
  HERDR_MANIFEST,
  registerHerdrExtension,
} = require("./extensions/builtin-herdr.cjs");
const { ARTIFACTS_MANIFEST } = require("./extensions/builtin-artifacts.cjs");
const { configureArtifactsSkill } = require("./artifacts-skill.cjs");
const { syncLocalBuiltinSkills } = require("./extensions/builtin-skills.cjs");
const {
  ORCHESTRATOR_MANIFEST,
  registerOrchestratorExtension,
  orchestratorNotice,
} = require("./orchestrator.cjs");
const {
  install,
  applicationPath,
  cleanupCompleted,
} = require("./installer.cjs");
let connections, preview, updates;
const agents = new AgentRegistry();
const claudeMcp = new ClaudeMcp({ home: os.homedir() });
const claudePlugins = new ClaudePlugins({ home: os.homedir() });
agents.on("event", (event) => send("agent-event", event));
const terminals = new Map(),
  terminalPending = new Map(),
  chats = new Map();
let mainWindow;
const root = path.join(__dirname, "..");
const dataDir = process.env.BRIDGE_DATA_DIR;
if (dataDir) app.setPath("userData", path.resolve(dataDir));
const extensions = new ExtensionManager({
  dataDir: app.getPath("userData"),
  builtins: [HERDR_MANIFEST, ORCHESTRATOR_MANIFEST, ARTIFACTS_MANIFEST],
  // Folders dropped here are read as JSON manifests, never executed. The
  // override exists so the desktop smoke can point at its own fixtures.
  localDir: process.env.SUSHIAI_EXTENSIONS_DIR
    ? path.resolve(root, process.env.SUSHIAI_EXTENSIONS_DIR)
    : // A sibling of the settings folder, not inside it: this one is meant to
      // be opened in Finder and edited by hand.
      path.join(app.getPath("userData"), "local-extensions"),
});
configureArtifactsSkill({
  isEnabled: () => extensions.isEnabled(ARTIFACTS_MANIFEST.id),
  subscribe: (listener) => extensions.onChange(listener),
  home: os.homedir(),
  codexAccountsDir: path.join(app.getPath("userData"), "codex-accounts"),
});
// After the persisted enabled state is loaded, so a disabled Artifacts
// extension does not get its skill installed (and a stale one is removed).
extensions.ready.then(() =>
  syncLocalBuiltinSkills(
    os.homedir(),
    undefined,
    path.join(app.getPath("userData"), "codex-accounts"),
  ),
);
const surfaceState = new SurfaceStateStore(app.getPath("userData"));
agents.register(
  new HermesProvider({
    userDataDir: app.getPath("userData"),
  }),
);
const modelProviders = new ModelProviders({
  userDataDir: app.getPath("userData"),
  safeStorage,
});
const codexAccounts = new CodexAccounts({
  userDataDir: app.getPath("userData"),
  codexBinary: () => executable("codex"),
  remoteExec: (endpoint, command) =>
    connections.exec(endpoint, command, { timeout: 8000 }),
});
const projects = new Projects({
  userDataDir: app.getPath("userData"),
  safeStorage,
});
const extraPath = [
  path.join(os.homedir(), ".local/bin"),
  "/opt/homebrew/bin",
  "/usr/local/bin",
  "/usr/bin",
  "/bin",
];
process.env.PATH = [
  ...new Set([...(process.env.PATH || "").split(":"), ...extraPath]),
].join(":");
function executable(name) {
  for (const dir of process.env.PATH.split(":")) {
    const full = path.join(dir, name);
    if (existsSync(full)) return full;
  }
  return null;
}
function directory(value) {
  if (
    typeof value !== "string" ||
    !path.isAbsolute(value) ||
    !existsSync(value)
  )
    throw new Error("Choose an existing project folder.");
  return value;
}
function id(value) {
  if (typeof value !== "string" || !value || value.length > 200)
    throw new Error("Invalid panel ID.");
  return value;
}
/** Writes a `claude --settings` file for a model profile; the caller owns
 * where it's used (a local pty's argv, or typed into a herdr pane). */
async function stageModelSettings(modelProfileId) {
  id(modelProfileId);
  return modelProviders.stageSettings(modelProfileId, os.tmpdir());
}
async function stageClaudeAccount(accountId) {
  id(accountId);
  return modelProviders.stageClaudeAccount(accountId, os.tmpdir());
}
function send(channel, value) {
  if (mainWindow && !mainWindow.isDestroyed())
    mainWindow.webContents.send(channel, value);
}
function handle(channel, callback) {
  ipcMain.handle(channel, async (event, ...args) => {
    if (
      event.sender !== mainWindow?.webContents ||
      event.senderFrame !== mainWindow.webContents.mainFrame
    )
      throw new Error("Untrusted IPC sender");
    return ipcResult(callback, args);
  });
}
// Connect / Retry on a host the daemon manager owns is a manager retry; the
// Herdr tunnel path only serves hosts the manager does not know.
const connectThroughDaemon =
  (original) =>
  async (endpoint, ...rest) => {
    if (typeof endpoint === "string" && endpoint.startsWith("ssh:")) {
      const host = endpoint.slice(4);
      if (daemonManager?.states().some((state) => state.host === host)) {
        const state = await daemonManager.retry(host);
        await connections.setAutoConnect(endpoint, true);
        return { connected: state.state === "ready", setup: "" };
      }
    }
    return original(endpoint, ...rest);
  };
registerProjectIpc({
  handle: (channel, callback) =>
    handle(
      channel,
      channel === "connections-connect"
        ? connectThroughDaemon(callback)
        : callback,
    ),
  getConnections: () => connections,
  getPreview: () => preview,
  getClaudeMcp: () => claudeMcp,
  terminals,
  terminalPending,
  projects,
});
let daemonManager = null;
// Host binaries and their manifest: app resources when packaged, else target/host.
const hostManifest = () =>
  requireHostManifest({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
  });
setHostManifestSource(hostManifest);
// The manager is created in whenReady; subscribers registered before that
// are served by one forwarding subscription made when it exists.
const daemonEventListeners = new Set();
const onDaemonEvent = (listener) => {
  daemonEventListeners.add(listener);
  return () => daemonEventListeners.delete(listener);
};
registerDaemonIpc({
  handle,
  send,
  getManager: () => daemonManager,
  onEvent: onDaemonEvent,
  attachmentsDir: path.join(app.getPath("temp"), "sushiai-attachments"),
  exec: (...args) => connections.exec(...args),
  installHost: (host) => {
    if (!daemonManager) throw new Error("daemon manager is not running");
    return createHostInstaller({
      manager: daemonManager,
      connections,
      manifest: hostManifest,
    }).install(host);
  },
  launch: createDaemonLaunch({
    manager: {
      request: (...args) => {
        if (!daemonManager) throw new Error("daemon manager is not running");
        return daemonManager.request(...args);
      },
    },
    projects,
    // `connections` is created in whenReady.
    connections: {
      inspect: (...args) => connections.inspect(...args),
      exec: (...args) => connections.exec(...args),
      hasShell: (endpoint) => connections.hasShell(endpoint),
    },
    modelProviders,
    codexAccounts,
  }).launch,
});
registerExtensionIpc({
  handle,
  getExtensions: () => extensions,
  getSurfaceState: () => surfaceState,
  announce: (change) => send("extensions-state-changed", change),
});
const herdrExtension = registerHerdrExtension({
  handle,
  getConnections: () => connections,
  id,
  send,
  executable,
});
registerWorkspaceSnapshot({
  ipcMain,
  handle,
  getMainWindow: () => mainWindow,
  userDataDir: () => app.getPath("userData"),
});
const terminalIpc = registerTerminalIpc({
  handle,
  send,
  app,
  pty,
  getConnections: () => connections,
  executable,
  directory,
  id,
  terminals,
  terminalPending,
  stageModelSettings,
  stageClaudeAccount,
  resolveClaudeAccount: (accountId) => {
    id(accountId);
    return modelProviders.resolveClaudeAccount(accountId);
  },
  resolveCodexAccount: (accountId, endpoint) => {
    id(accountId);
    return codexAccounts.resolve(accountId, endpoint);
  },
  resolveModel: (profileId) => {
    id(profileId);
    return modelProviders.resolveEnv(profileId);
  },
  projects,
  // The test harness runs a fake ssh (see SUSHIAI_TEST_SSH above).
  sshBinary: (testMode.hidden && process.env.SUSHIAI_TEST_SSH) || undefined,
});
const chatIpc = registerChatIpc({
  handle,
  send,
  getConnections: () => connections,
  executable,
  directory,
  id,
  chats,
});
registerAppIpc({
  handle,
  app,
  dialog,
  shell,
  getMainWindow: () => mainWindow,
  getConnections: () => connections,
  getUpdates: () => updates,
  agents,
  claudeMcp,
  claudePlugins,
  modelProviders,
  codexAccounts,
  executable,
  scanLocalSkills,
  manageSkill,
  userDataDir: () => app.getPath("userData"),
});
let orchestrator;
let devRestart = false;
let closeCoreWatch = null;
let closePresentingWatch = null;
const mascot = registerMascot({
  ipcMain,
  BrowserWindow,
  screen,
  root,
  policy: testMode.mascot,
  devURL: process.env.BRIDGE_DEV_URL,
  getService: () => orchestrator,
  showMainWindow: () => attention.showWindow(),
  send,
  restart: () => {
    devRestart = true;
    app.quit();
  },
});
function coreUpdated(file) {
  console.log(`[dev] electron/${file} changed - restart from the mascot`);
  mascot.add({
    kind: "core-update",
    title: "sushiAI core",
    body: "Core updated - restart?",
  });
}
if (process.env.BRIDGE_DEV_URL)
  closeCoreWatch = watchCore({ dir: __dirname, onChange: coreUpdated });
// A test run owns neither the owner's keyboard nor their screen state.
const mascotShortcut = createMascotShortcut({
  globalShortcut,
  toggle: () => mascot.toggle(),
});
handle("mascot-shortcut-status", () => mascotShortcut.status());
const attention = registerAttentionIpc({
  handle,
  send,
  app,
  getMainWindow: () => mainWindow,
  userDataDir: app.getPath("userData"),
  trayIconPath: path.join(root, "dist/trayTemplate.png"),
  mascot,
  hidden: testMode.hidden,
  onPreferences: (preferences) => {
    if (!testMode.test) mascotShortcut.sync(preferences.mascotShortcut);
  },
});
orchestrator = registerOrchestratorExtension({
  handle,
  extensions,
  send,
  notify: (notice) => attention.notifyTask(notice),
  onTask: (task) => mascot.onTask(task),
  dataDir: path.join(app.getPath("userData"), "orchestrator"),
  root,
  resourcesPath: process.resourcesPath,
  packaged: app.isPackaged,
  getClaudeMcp: () => claudeMcp,
  getModelProviders: () => modelProviders,
  getProjects: () => projects,
  stopDaemonOnQuit: testMode.test,
  getConnections: () => connections,
  userDataDir: app.getPath("userData"),
  hostsChanged: () => send("orchestrator-hosts-changed"),
});
// Turning the orchestrator off also drops the notices it already queued.
extensions.onChange((id, enabled) => {
  if (id === ORCHESTRATOR_MANIFEST.id && !enabled) mascot.clear();
});
function validWebURL(value) {
  try {
    return ["http:", "https:"].includes(new URL(value).protocol);
  } catch {
    return false;
  }
}
app.whenReady().then(async () => {
  // A hidden test run may hand in a fake ssh (evidence and smoke runs).
  const fakeSsh = testMode.hidden ? process.env.SUSHIAI_TEST_SSH : "";
  connections = new Connections(
    app.getPath("userData"),
    fakeSsh ? { ssh: fakeSsh } : undefined,
  );
  await connections.init();
  // The local machine gets what sessions need (Herdr running, Claude Code, Codex) on
  // its own; a test run never installs anything.
  if (!testMode.test)
    void setupHost(connections, "local")
      .then((states) => {
        const summary = setupSummary(states);
        if (summary) console.log(`Environment: ${summary}`);
      })
      .catch((error) => console.error("Environment setup failed:", error));
  // A test run starts a daemon only when it brings its own SUSHIAI_HOME.
  if (!testMode.test || process.env.SUSHIAI_HOME) {
    const local = createLocalConnector({
      appVersion: app.getVersion(),
      isPackaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      log: (message) => console.log(`daemon: ${message}`),
    });
    // One daemon host per connection profile; saving or deleting a profile
    // adds, replaces or removes its host while the app runs.
    const connectorMap = (profiles) => ({
      local,
      ...remoteConnectors(profiles, {
        ssh: connections.ssh,
        args: (profile) => connections.args(profile),
      }),
    });
    daemonManager = createDaemonManager({
      connectors: connectorMap(connections.profiles),
      powerMonitor,
      log: (message) => console.log(`daemon: ${message}`),
    });
    daemonManager.on("event", (event) => {
      for (const listener of [...daemonEventListeners]) listener(event);
    });
    forwardDaemonEvents(daemonManager, (channel, value) => {
      for (const window of BrowserWindow.getAllWindows())
        if (!window.isDestroyed()) window.webContents.send(channel, value);
    });
    connections.onProfilesChange((profiles) =>
      daemonManager?.setConnectors(connectorMap(profiles)),
    );
    daemonManager.start();
  }
  await orchestrator.start();
  // Sleep/wake can drop every SSH tunnel at once - retry them all rather than
  // waiting for each one's own backoff timer to come back around.
  powerMonitor.on("resume", () => connections.retryAutoConnect());
  updates = new Updates({
    directory: app.getPath("userData"),
    currentVersion: app.getVersion(),
    canInstall: app.isPackaged && process.platform === "darwin",
    installer: (release, dmg) =>
      install({
        directory: path.join(app.getPath("userData"), "updates"),
        release,
        dmg,
        packaged: app.isPackaged,
        onReady: () => app.quit(),
      }),
    openPath: (file) => shell.openPath(file),
    openExternal: (url) => shell.openExternal(url),
    onChange: (state) => send("updates-state", state),
    automatic: app.isPackaged && !testMode.hidden,
  });
  await updates.init();
  await attention.init();
  preview = new PreviewServer(connections);
  await preview.start();
  if (process.platform === "darwin" && testMode.hidden) app.dock.hide();
  else if (
    process.platform === "darwin" &&
    existsSync(path.join(root, "dist/sushi-dock.png"))
  )
    app.dock.setIcon(path.join(root, "dist/sushi-dock.png"));
  session.defaultSession.setPermissionRequestHandler((_, __, callback) =>
    callback(false),
  );
  const savedWindow = loadWindowState(app.getPath("userData"));
  const startBounds = savedWindow
    ? clampBounds(
        savedWindow,
        screen.getAllDisplays(),
        screen.getPrimaryDisplay(),
      )
    : DEFAULT_BOUNDS;
  mainWindow = new BrowserWindow({
    ...startBounds,
    minWidth: 600,
    minHeight: 440,
    title: "sushiAI",
    backgroundColor: "#0b0b0b",
    titleBarStyle: "hidden",
    trafficLightPosition: { x: 14, y: 14 },
    ...testMode.windowOptions,
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      webviewTag: true,
      ...testMode.windowOptions.webPreferences,
    },
  });
  // Evidence seam: pushes a task through the real notice path (no daemon).
  if (process.env.SUSHIAI_TEST_MASCOT === "1")
    globalThis.__sushiaiMascot = {
      notify: (task) => attention.notifyTask(orchestratorNotice(task)),
      coreUpdated: () => coreUpdated("<seam>"),
      queue: () => mascot.snapshot(),
      window: () => mascot.getWindow(),
      workArea: () => screen.getPrimaryDisplay().workArea,
      toggle: () => mascot.toggle(),
      setPresenting: (value) => mascot.setPresenting(value),
    };
  // A test run owns neither the owner's keyboard nor their screen state.
  if (!testMode.test) {
    closePresentingWatch = watchPresenting({
      screen,
      onChange: (presenting) => mascot.setPresenting(presenting),
    });
  }
  if (savedWindow && !testMode.hidden) {
    if (savedWindow.isFullScreen) mainWindow.setFullScreen(true);
    else if (savedWindow.isMaximized) mainWindow.maximize();
  }
  const saveWindow = () => {
    if (mainWindow.isDestroyed()) return;
    const normalBounds = mainWindow.getNormalBounds();
    try {
      saveWindowState(app.getPath("userData"), {
        bounds: normalBounds,
        displayId: screen.getDisplayMatching(normalBounds).id,
        isMaximized: mainWindow.isMaximized(),
        isFullScreen: mainWindow.isFullScreen(),
      });
    } catch {
      // A failed save must never block closing.
    }
  };
  let saveTimer = null;
  const saveWindowSoon = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(saveWindow, 500);
  };
  for (const name of [
    "resize",
    "move",
    "maximize",
    "unmaximize",
    "enter-full-screen",
    "leave-full-screen",
  ])
    mainWindow.on(name, saveWindowSoon);
  mainWindow.on("closed", () => mascot.destroy());
  mainWindow.on("close", (event) => {
    clearTimeout(saveTimer);
    saveWindow();
    if (attention.handleWindowClose(mainWindow)) event.preventDefault();
  });
  mainWindow.webContents.on("will-navigate", (event) => event.preventDefault());
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  mainWindow.webContents.on(
    "will-attach-webview",
    (event, preferences, params) => {
      delete preferences.preload;
      preferences.nodeIntegration = false;
      preferences.contextIsolation = true;
      preferences.sandbox = true;
      if (!validWebURL(params.src)) event.preventDefault();
    },
  );
  app.on("web-contents-created", (_, contents) => {
    if (contents.getType() !== "webview") return;
    contents.session.setPermissionRequestHandler((_, __, callback) =>
      callback(false),
    );
    contents.setWindowOpenHandler(() => ({ action: "deny" }));
    contents.on("will-navigate", (event, url) => {
      if (!validWebURL(url)) event.preventDefault();
    });
    contents.on("will-redirect", (event, url) => {
      if (!validWebURL(url)) event.preventDefault();
    });
  });
  Menu.setApplicationMenu(
    Menu.buildFromTemplate([
      {
        label: "sushiAI",
        submenu: [
          { role: "about" },
          { type: "separator" },
          { role: "hide" },
          { role: "quit" },
        ],
      },
      {
        label: "Edit",
        submenu: [
          { role: "undo" },
          { role: "redo" },
          { type: "separator" },
          { role: "cut" },
          { role: "copy" },
          { role: "paste" },
          { role: "selectAll" },
        ],
      },
      {
        label: "View",
        submenu: [
          { role: "reload" },
          { role: "toggleDevTools" },
          { role: "togglefullscreen" },
        ],
      },
      {
        label: "Window",
        submenu: [{ role: "close" }],
      },
    ]),
  );
  mainWindow.webContents.once("did-finish-load", async () => {
    if (app.isPackaged) {
      try {
        await cleanupCompleted(
          path.join(app.getPath("userData"), "updates", "installed.json"),
          await fs.realpath(applicationPath()),
          app.getVersion(),
        );
      } catch {}
    }
  });
  const devURL = process.env.BRIDGE_DEV_URL;
  if (devURL === "http://127.0.0.1:5173") mainWindow.loadURL(devURL);
  else mainWindow.loadFile(path.join(root, "dist/index.html"));
});
app.on("window-all-closed", () => app.quit());
app.on("activate", () => attention.showWindow());
let quitReady = false;
app.on("will-quit", (event) => {
  mascotShortcut.close();
  globalShortcut.unregisterAll();
  closePresentingWatch?.();
  if (!devRestart) return;
  event.preventDefault();
  app.exit(DEV_RESTART_EXIT_CODE);
});
app.on("before-quit", (event) => {
  attention.setQuitting(true);
  if (quitReady) return;
  event.preventDefault();
  closeCoreWatch?.();
  updates?.close();
  attention.close();
  mascot.destroy();
  // The daemon may take several seconds to stop its agents; the app should
  // look closed meanwhile, not frozen.
  for (const window of BrowserWindow.getAllWindows())
    if (!window.isDestroyed()) window.hide();
  preview?.close();
  terminalIpc.close();
  herdrExtension.close();
  daemonManager?.close();
  for (const pending of terminalPending.values()) pending.cancelled = true;
  for (const terminal of terminals.values())
    if (!terminal.exited || terminal.source === "herdr") terminal.proc.kill();
  chatIpc.close();
  Promise.allSettled([
    Promise.resolve(connections?.close()),
    agents.close(),
    orchestrator.quit(),
  ]).finally(() => {
    quitReady = true;
    app.quit();
  });
});
