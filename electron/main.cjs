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
  Notification,
} = require("electron");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs/promises");
const { existsSync } = require("node:fs");
const { Connections } = require("./connections.cjs");
const { readStore, writeStore } = require("./app-db.cjs");
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
const {
  setupHost,
  setupSummary,
  requireHostManifest,
  createHostInstaller,
  serializePerHost,
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
const { watchHostTools } = require("./daemon/host-tools.cjs");
const { registerAttentionIpc } = require("./attention.cjs");
const {
  registerWorkspaceSnapshot,
  savedWorkspaces,
} = require("./workspace-snapshot.cjs");
const { createCatalogSync, revStore } = require("./catalog-sync.cjs");
const {
  DEFAULT_BOUNDS,
  loadWindowState,
  saveWindowState,
  clampBounds,
} = require("./window-state.cjs");
const { registerMascot, watchPresenting } = require("./mascot.cjs");
const { createMascotShortcut } = require("./mascot-shortcut.cjs");
const { createNotices } = require("./extensions/notices.cjs");
const { createOrchestratorNotices } = require("./orchestrator-notices.cjs");
const { DEV_RESTART_EXIT_CODE, watchCore } = require("./dev-restart.cjs");
const { testWindow } = require("./test-window.cjs");

const testMode = testWindow();
const { SurfaceStateStore } = require("./extensions/surface-state.cjs");
const { ExtensionManager } = require("./extensions/extension-manager.cjs");
const { ARTIFACTS_MANIFEST } = require("./extensions/builtin-artifacts.cjs");
const { configureArtifactsSkill } = require("./artifacts-skill.cjs");
const { syncLocalBuiltinSkills } = require("./extensions/builtin-skills.cjs");
const {
  ORCHESTRATOR_MANIFEST,
} = require("./extensions/builtin-orchestrator.cjs");
const {
  registerOrchestratorExtension,
  createModuleSwitch,
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
const chats = new Map();
let mainWindow;
const root = path.join(__dirname, "..");
const dataDir = process.env.BRIDGE_DATA_DIR;
if (dataDir) app.setPath("userData", path.resolve(dataDir));
const extensions = new ExtensionManager({
  dataDir: app.getPath("userData"),
  builtins: [ORCHESTRATOR_MANIFEST, ARTIFACTS_MANIFEST],
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
extensions.ready.then(() =>
  surfaceState.forgetRetired(new Set(extensions.manifests.keys())),
);
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
// Connect / Retry / Disconnect on a host the daemon manager owns go to the
// manager ("local" is This Mac). Connecting a host that is ready or connecting
// does nothing.
const daemonHost = (endpoint) => {
  if (endpoint === "local") return daemonManager ? "local" : null;
  if (typeof endpoint !== "string" || !endpoint.startsWith("ssh:")) return null;
  const host = endpoint.slice(4);
  return daemonManager?.states().some((state) => state.host === host)
    ? host
    : null;
};
// The auto-connect flag is set before the (slow) retry is awaited: a
// Disconnect clicked meanwhile clears it afterwards, and the late connect
// result must not turn it back on.
const connectHost = async (endpoint) => {
  const host = daemonHost(endpoint);
  if (!host) throw new Error("The sushiai daemon is not running.");
  if (host !== "local") await connections.setAutoConnect(endpoint, true);
  let state = daemonManager.states().find((item) => item.host === host);
  if (state.state !== "ready" && state.state !== "connecting")
    state = await daemonManager.retry(host);
  return { connected: state.state === "ready" };
};
registerProjectIpc({
  handle,
  connectHost,
  // The manager closes the host's terminals when it reports "disconnected".
  onDisconnect: (endpoint) => {
    const host = daemonHost(endpoint);
    if (host) daemonManager.disconnect(host);
  },
  getConnections: () => connections,
  getPreview: () => preview,
  getClaudeMcp: () => claudeMcp,
  projects,
});
let daemonManager = null;
let localConnector = null;
let catalogSync = null;
let hostTools = null;
// Host binaries and their manifest: app resources when packaged, else target/host.
const hostManifest = () =>
  requireHostManifest({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
  });
// The one installer: tools, skills and sushiai.
const setupDaemonHost = serializePerHost((endpoint, options) =>
  setupHost(connections, endpoint, options),
);
const installDaemonHost = (host) => {
  if (!daemonManager) throw new Error("daemon manager is not running");
  return createHostInstaller({
    manager: daemonManager,
    connections,
    manifest: hostManifest,
    setup: setupDaemonHost,
    restartLocal: () => localConnector.restart(),
    markSetup: (name) => hostTools?.mark(name),
  }).install(host);
};
// The manager is created in whenReady; subscribers registered before that
// are served by one forwarding subscription made when it exists.
const daemonEventListeners = new Set();
let daemonIpc = null;
const onDaemonEvent = (listener) => {
  daemonEventListeners.add(listener);
  return () => daemonEventListeners.delete(listener);
};
daemonIpc = registerDaemonIpc({
  handle,
  send,
  getManager: () => daemonManager,
  onEvent: onDaemonEvent,
  attachmentsDir: path.join(app.getPath("temp"), "sushiai-attachments"),
  exec: (...args) => connections.exec(...args),
  installHost: installDaemonHost,
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
registerWorkspaceSnapshot({
  ipcMain,
  handle,
  getMainWindow: () => mainWindow,
  userDataDir: () => app.getPath("userData"),
  onWrite: () => catalogSync?.notifyChanged(),
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
  act: (source, key, actionId, text) =>
    notices.act(source, key, actionId, text),
  showMainWindow: () => attention.showWindow(),
  send,
});
function coreUpdated(file) {
  console.log(`[dev] electron/${file} changed - restart from the mascot`);
  notices.publish("app", {
    key: "core-update",
    kind: "info",
    label: "Update",
    title: "sushiAI core",
    body: "Core updated - restart?",
    sticky: true,
    actions: [{ id: "restart", label: "Restart", emphasis: "primary" }],
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
const notices = createNotices({
  preferences: () => attention.getPreferences(),
  mascot,
  showWindow: () => attention.showWindow(),
  Notification,
  icon: () => attention.mascotImage(),
});
notices.register("app", async (_key, actionId) => {
  if (actionId !== "restart") return;
  devRestart = true;
  app.quit();
});
const orchestratorNotices = createOrchestratorNotices({
  notices,
  getService: () => orchestrator,
  showWindow: () => attention.showWindow(),
  send,
});
orchestrator = registerOrchestratorExtension({
  handle,
  extensions,
  send,
  notify: (notice) => orchestratorNotices.publish(notice),
  onTask: (task) => orchestratorNotices.onTask(task),
  getClaudeMcp: () => claudeMcp,
  getModelProviders: () => modelProviders,
  getProjects: () => projects,
  getConnections: () => connections,
  getManager: () => daemonManager,
  installHost: installDaemonHost,
  // Enabling the Orchestrator registers its module on each host. A test run
  // must never write the owner's Claude or Codex files.
  moduleSwitch: testMode.test
    ? null
    : createModuleSwitch({
        getManager: () => daemonManager,
        runLocal: (args) => localConnector.runCli(args),
        restartLocal: () => localConnector.restart(),
        exec: (...args) => connections.exec(...args),
      }),
  log: (message) => console.log(message),
  userDataDir: app.getPath("userData"),
  hostsChanged: () => send("orchestrator-hosts-changed"),
});
// Turning an extension off also drops the notices it already queued.
extensions.onChange((id, enabled) => {
  if (!enabled) notices.clear(id);
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
  // The local machine gets what sessions need (Claude Code, Codex) on its own; a test run never installs anything.
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
      // A standalone orchd of a previous build is stopped before the daemon starts.
      legacyOrchdDir: path.join(app.getPath("userData"), "orchestrator"),
      log: (message) => console.log(`daemon: ${message}`),
    });
    localConnector = local;
    // One daemon host per connection profile; saving or deleting a profile
    // adds, replaces or removes its host while the app runs.
    const connectorMap = (profiles) => ({
      local,
      ...remoteConnectors(profiles, {
        ssh: connections.ssh,
        args: (profile) => connections.args(profile),
        knownHostsFile: connections.knownHostsFile,
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
    // A host that is removed or disconnected ends its terminals.
    daemonManager.on("state", (state) => {
      if (state.reason === "removed" || state.reason === "disconnected")
        void daemonIpc?.terminals.closeHost(state.host);
    });
    hostTools = watchHostTools({
      manager: daemonManager,
      connections,
      store: {
        read: () => readStore(app.getPath("userData"), "host-tools") || {},
        write: (value) =>
          writeStore(app.getPath("userData"), "host-tools", value),
      },
      setup: setupDaemonHost,
      manifest: () => {
        try {
          return hostManifest();
        } catch {
          return null;
        }
      },
      log: (message) => console.log(`daemon: ${message}`),
    });
    catalogSync = createCatalogSync({
      manager: daemonManager,
      projects,
      workspaces: () => savedWorkspaces(app.getPath("userData")),
      store: revStore(app.getPath("userData")),
    });
    daemonManager.start();
  }
  await orchestrator.start();
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
      notify: (task) => orchestratorNotices.publish(orchestratorNotice(task)),
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
  // A reloaded or crashed renderer never acks its terminals: release them.
  const releaseDaemonPanels = () => void daemonIpc?.terminals.detachAll();
  mainWindow.webContents.on("did-start-navigation", (details) => {
    if (details.isMainFrame && !details.isSameDocument) releaseDaemonPanels();
  });
  mainWindow.webContents.on("render-process-gone", releaseDaemonPanels);
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
  catalogSync?.stop();
  daemonManager?.close();
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
