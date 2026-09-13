const {
  app,
  BrowserWindow,
  ipcMain,
  dialog,
  Menu,
  session,
  shell,
  safeStorage,
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
const { AgentRegistry } = require("./agents/registry.cjs");
const { HermesProvider } = require("./agents/hermes-provider.cjs");
const { registerProjectIpc } = require("./ipc/projects.cjs");
const { registerTerminalIpc } = require("./ipc/terminals.cjs");
const { registerChatIpc } = require("./ipc/chat.cjs");
const { registerAppIpc } = require("./ipc/app.cjs");
const { registerExtensionIpc } = require("./ipc/extensions.cjs");
const { SurfaceStateStore } = require("./extensions/surface-state.cjs");
const { ExtensionManager } = require("./extensions/extension-manager.cjs");
const {
  HERDR_MANIFEST,
  registerHerdrExtension,
} = require("./extensions/builtin-herdr.cjs");
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
  builtins: [HERDR_MANIFEST],
  // Folders dropped here are read as JSON manifests, never executed. The
  // override exists so the desktop smoke can point at its own fixtures.
  localDir: process.env.SUSHIAI_EXTENSIONS_DIR
    ? path.resolve(root, process.env.SUSHIAI_EXTENSIONS_DIR)
    : // A sibling of the settings folder, not inside it: this one is meant to
      // be opened in Finder and edited by hand.
      path.join(app.getPath("userData"), "local-extensions"),
});
const surfaceState = new SurfaceStateStore(
  path.join(app.getPath("userData"), "extensions", "state"),
);
agents.register(
  new HermesProvider({
    activityFile: path.join(
      app.getPath("userData"),
      "agents",
      "hermes-activity.json",
    ),
  }),
);
const modelProviders = new ModelProviders({
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
    return callback(...args);
  });
}
registerProjectIpc({
  handle,
  getConnections: () => connections,
  getPreview: () => preview,
  terminals,
  terminalPending,
});
registerExtensionIpc({
  handle,
  getExtensions: () => extensions,
  getSurfaceState: () => surfaceState,
  announce: (change) => send("extensions-state-changed", change),
});
registerHerdrExtension({ handle, getConnections: () => connections, id });
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
  executable,
  scanLocalSkills,
  manageSkill,
  userDataDir: () => app.getPath("userData"),
  stageModelSettings,
});
function validWebURL(value) {
  try {
    return ["http:", "https:"].includes(new URL(value).protocol);
  } catch {
    return false;
  }
}
app.whenReady().then(async () => {
  connections = new Connections(app.getPath("userData"));
  await connections.init();
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
    automatic: app.isPackaged && process.env.SUSHIAI_TEST_HEADLESS !== "1",
  });
  await updates.init();
  preview = new PreviewServer(connections);
  await preview.start();
  if (
    process.platform === "darwin" &&
    existsSync(path.join(root, "dist/sushi-dock.png"))
  )
    app.dock.setIcon(path.join(root, "dist/sushi-dock.png"));
  session.defaultSession.setPermissionRequestHandler((_, __, callback) =>
    callback(false),
  );
  mainWindow = new BrowserWindow({
    width: 1380,
    height: 880,
    minWidth: 600,
    minHeight: 440,
    title: "sushiAI",
    show: process.env.SUSHIAI_TEST_HEADLESS !== "1",
    backgroundColor: "#0b0b0b",
    titleBarStyle: "hidden",
    trafficLightPosition: { x: 14, y: 14 },
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      webviewTag: true,
      backgroundThrottling: process.env.SUSHIAI_TEST_HEADLESS !== "1",
    },
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
let quitReady = false;
app.on("before-quit", (event) => {
  if (quitReady) return;
  event.preventDefault();
  updates?.close();
  preview?.close();
  terminalIpc.close();
  for (const pending of terminalPending.values()) pending.cancelled = true;
  for (const terminal of terminals.values())
    if (!terminal.exited) terminal.proc.kill();
  chatIpc.close();
  Promise.allSettled([
    Promise.resolve(connections?.close()),
    agents.close(),
  ]).finally(() => {
    quitReady = true;
    app.quit();
  });
});
