const {
  app,
  BrowserWindow,
  ipcMain,
  dialog,
  Menu,
  session,
  shell,
} = require("electron");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs/promises");
const { existsSync } = require("node:fs");
const { spawn } = require("node:child_process");
const pty = require("node-pty");
const { request, inputCommands } = require("./herdr.cjs");
const { Connections } = require("./connections.cjs");
const { PreviewServer } = require("./preview.cjs");
const { openHerdrStream, detectAgent } = require("./terminal-stream.cjs");
const { quote } = require("./connections.cjs");
const { Updates } = require("./updates.cjs");
const {
  install,
  applicationPath,
  cleanupCompleted,
} = require("./installer.cjs");
let connections, preview, updates;
const terminals = new Map(),
  terminalPending = new Map(),
  chats = new Map(),
  inputQueues = new Map();
let mainWindow;
const root = path.join(__dirname, "..");
const dataDir = process.env.BRIDGE_DATA_DIR;
if (dataDir) app.setPath("userData", path.resolve(dataDir));
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
    return callback(...args);
  });
}
const allowedHerdr = new Set([
  "ping",
  "session.snapshot",
  "workspace.create",
  "workspace.rename",
  "pane.rename",
  "pane.split",
  "pane.read",
  "pane.send_text",
  "pane.send_keys",
  "pane.send_input",
  "plugin.list",
  "pane.close",
  "workspace.close",
]);
const detectionTimer = setInterval(() => {
  for (const [panelId, entry] of terminals) {
    if (entry.exited || entry.source !== "pty") continue;
    let foreground = "";
    try {
      foreground = entry.remote
        ? entry.command || ""
        : entry.proc.process || "";
    } catch {}
    const agent = detectAgent(foreground, entry.title);
    if (agent !== entry.lastAgent) {
      entry.lastAgent = agent;
      send("terminal-data", { panelId, data: "", agent });
    }
  }
}, 1200);
detectionTimer.unref();
handle("herdr", async (endpoint, method, params = {}) => {
  const socketPath = await connections.socket(endpoint);
  if (
    typeof socketPath !== "string" ||
    !path.isAbsolute(socketPath) ||
    !allowedHerdr.has(method)
  )
    throw new Error("Invalid Herdr request");
  if (method === "pane.send_input" && params.raw !== undefined) {
    if (typeof params.raw !== "string" || params.raw.length > 1000000)
      throw new Error("Input too large");
    const queueKey = socketPath + ":" + id(params.pane_id);
    const next = (inputQueues.get(queueKey) || Promise.resolve())
      .catch(() => {})
      .then(async () => {
        for (const command of inputCommands(params.raw))
          await request(socketPath, "pane.send_input", {
            pane_id: params.pane_id,
            ...command,
          });
      });
    inputQueues.set(queueKey, next);
    try {
      await next;
      return {};
    } finally {
      if (inputQueues.get(queueKey) === next) inputQueues.delete(queueKey);
    }
  }
  return request(socketPath, method, params);
});
handle("updates-state", () => updates.snapshot());
handle("updates-check", () => updates.check());
handle("updates-download", () => updates.download());
handle("updates-configure", (settings) => updates.configure(settings));
handle("updates-open", () => updates.openInstaller());
handle("updates-install", () => updates.install());
handle("updates-release-page", () => updates.releasePage());
handle("connections-list", () => connections.list());
handle("connections-save", (profile) => connections.save(profile));
async function disconnectEndpoint(endpoint) {
  for (const pending of terminalPending.values())
    if (pending.endpoint === endpoint) pending.cancelled = true;
  for (const terminal of terminals.values())
    if (terminal.endpoint === endpoint && !terminal.exited)
      terminal.proc.kill();
  await connections.disconnect(endpoint);
}
handle("connections-delete", async (endpoint) => {
  await disconnectEndpoint(endpoint);
  await connections.delete(endpoint);
});
handle("connections-connect", async (endpoint) => {
  await connections.socket(endpoint);
  return { connected: true };
});
handle("connections-disconnect", disconnectEndpoint);
handle("connections-forward", async (endpoint, url) => {
  const parsed = new URL(url);
  if (
    !["http:", "https:"].includes(parsed.protocol) ||
    !["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname)
  )
    throw new Error("Forwarding supports only a remote localhost URL.");
  const local = await connections.forward(
    endpoint,
    Number(parsed.port) || (parsed.protocol === "https:" ? 443 : 80),
  );
  parsed.hostname = "127.0.0.1";
  parsed.port = String(local);
  return parsed.href;
});
handle("project-inspect", (endpoint, options) => {
  if (
    !["list", "read", "write", "git", "diff", "home"].includes(
      options?.operation,
    )
  )
    throw new Error("Unknown project operation");
  return connections.inspect(endpoint, options);
});
handle("project-preview", async (endpoint, root, file) => {
  await connections.inspect(endpoint, { operation: "read", root, path: file });
  return preview.grant(endpoint, root, file);
});
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
handle("choose-directory", async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ["openDirectory", "createDirectory"],
  });
  return result.canceled ? null : result.filePaths[0];
});
handle("window", (action) => {
  if (action === "minimize") mainWindow.minimize();
  if (action === "maximize")
    mainWindow.setFullScreen(!mainWindow.isFullScreen());
  if (action === "close") mainWindow.close();
});
handle(
  "terminal-open",
  async ({
    panelId,
    cwd,
    command,
    cols = 80,
    rows = 24,
    endpoint,
    herdrId,
  }) => {
    id(panelId);
    if (terminals.has(panelId))
      return {
        history: terminals.get(panelId).history,
        exited: terminals.get(panelId).exited,
      };
    if (herdrId) {
      id(herdrId);
      if (terminalPending.has(panelId)) {
        const pending = terminalPending.get(panelId);
        const entry = await pending.promise;
        return { history: entry.history, exited: entry.exited };
      }
      const pending = { cancelled: false, endpoint };
      terminalPending.set(panelId, pending);
      pending.promise = openHerdrStream({
        endpoint,
        panelId,
        target: herdrId,
        cols,
        rows,
        connections,
        binary: executable("herdr"),
        send: (channel, event) => {
          if (!pending.cancelled) send(channel, event);
        },
      });
      try {
        const entry = await pending.promise;
        if (pending.cancelled) {
          entry.proc.kill();
          return { history: "", exited: true };
        }
        terminals.set(panelId, entry);
        entry.opening = pending;
        entry.endpoint = endpoint;
        return { history: "" };
      } finally {
        if (terminalPending.get(panelId) === pending)
          terminalPending.delete(panelId);
      }
    }
    const remote = endpoint?.startsWith("ssh:")
      ? connections.get(endpoint)
      : null;
    if (!remote) directory(cwd);
    else if (typeof cwd !== "string" || !cwd.startsWith("/"))
      throw new Error("Choose an absolute remote project folder.");
    if (
      command &&
      !["claude", "codex", "gemini", "cursor-agent"].includes(command)
    )
      throw new Error("Unsupported agent");
    let binary = command
      ? executable(command)
      : process.env.SHELL || "/bin/zsh";
    let args = command ? [] : ["-l"];
    if (remote) {
      binary = "/usr/bin/ssh";
      args = [
        ...connections.args(remote),
        "-tt",
        remote.host,
        `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; cd ${quote(cwd)} && exec ${command ? quote(command) : '"${SHELL:-/bin/sh}" -l'}`,
      ];
    }
    if (!binary)
      throw new Error(
        `${command} is not installed. Install it and sign in from a terminal first.`,
      );
    const proc = pty.spawn(binary, args, {
      name: "xterm-256color",
      cols: Math.max(10, Math.min(500, cols)),
      rows: Math.max(3, Math.min(300, rows)),
      cwd: remote ? os.homedir() : cwd,
      env: { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor" },
    });
    const entry = {
      proc,
      history: "",
      exited: false,
      source: "pty",
      lastAgent: undefined,
      title: "",
      remote: !!remote,
      command,
      endpoint,
    };
    terminals.set(panelId, entry);
    proc.onData((data) => {
      const titles = [
        ...data.matchAll(/\x1b\](?:0|2);([^\x07\x1b]*)(?:\x07|\x1b\\)/g),
      ];
      if (titles.length) entry.title = titles.at(-1)[1];
      entry.history = (entry.history + data).slice(-1000000);
      send("terminal-data", { panelId, data });
    });
    proc.onExit(({ exitCode }) => {
      entry.exited = true;
      send("terminal-data", {
        panelId,
        exitCode,
        data: `\r\n\x1b[90mProcess exited (${exitCode}). Close this panel to start a new session.\x1b[0m\r\n`,
      });
    });
    return { history: "" };
  },
);
handle("terminal-scroll", (panelId, direction, lines) => {
  if (
    ["up", "down"].includes(direction) &&
    Number.isInteger(lines) &&
    lines > 0 &&
    lines < 1000
  )
    terminals.get(id(panelId))?.proc.scroll?.(direction, lines);
});
handle("terminal-write", (panelId, data) => {
  if (typeof data !== "string" || data.length > 1000000)
    throw new Error("Invalid input");
  terminals.get(id(panelId))?.proc.write(data);
});
handle("terminal-resize", (panelId, cols, rows) => {
  if (
    !Number.isInteger(cols) ||
    !Number.isInteger(rows) ||
    cols < 10 ||
    rows < 3 ||
    cols > 500 ||
    rows > 300
  )
    return;
  const terminal = terminals.get(id(panelId));
  if (terminal && !terminal.exited) terminal.proc.resize(cols, rows);
});
handle("terminal-close", (panelId) => {
  const pending = terminalPending.get(id(panelId));
  if (pending) {
    pending.cancelled = true;
    terminalPending.delete(panelId);
  }
  const terminal = terminals.get(id(panelId));
  if (terminal?.opening) terminal.opening.cancelled = true;
  if (terminal && !terminal.exited) terminal.proc.kill();
  terminals.delete(panelId);
});

function stopChat(panelId) {
  const proc = chats.get(panelId);
  if (proc) {
    try {
      process.kill(-proc.pid, "SIGTERM");
    } catch {
      proc.kill();
    }
  }
}
handle("chat-cancel", (panelId) => stopChat(id(panelId)));
handle("chat", ({ panelId, cwd, agent, messages, endpoint }) => {
  id(panelId);
  const remote = endpoint?.startsWith("ssh:")
    ? connections.get(endpoint)
    : null;
  if (!remote) directory(cwd);
  else if (typeof cwd !== "string" || !cwd.startsWith("/"))
    throw new Error("Choose an absolute remote project folder.");
  if (chats.has(panelId)) throw new Error("A response is already running.");
  if (!["claude", "codex"].includes(agent))
    throw new Error("Choose Claude Code or Codex.");
  const binary = remote ? "/usr/bin/ssh" : executable(agent);
  if (!binary)
    throw new Error(
      `${agent} is not installed. Install and sign in to the CLI first.`,
    );
  if (
    !Array.isArray(messages) ||
    messages.length > 200 ||
    messages.some(
      (m) =>
        !["user", "assistant"].includes(m.role) || typeof m.text !== "string",
    )
  )
    throw new Error("Invalid conversation");
  const prompt = messages.map((m) => `${m.role}: ${m.text}`).join("\n\n");
  if (prompt.length > 200000)
    throw new Error("Conversation is too long. Start a new chat.");
  // Keep the prompt off argv and preserve each CLI's ordinary permission policy.
  let args =
    agent === "claude"
      ? ["--print", "--output-format", "text"]
      : ["exec", "--json", "--skip-git-repo-check", "-"];
  if (remote)
    args = [
      ...connections.args(remote),
      remote.host,
      `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; cd ${quote(cwd)} && exec ${quote(agent)} ${args.map(quote).join(" ")}`,
    ];
  const proc = spawn(binary, args, {
    cwd: remote ? os.homedir() : cwd,
    env: process.env,
    stdio: ["pipe", "pipe", "pipe"],
    detached: true,
  });
  chats.set(panelId, proc);
  let buffered = "",
    errorOutput = "";
  proc.stdout.setEncoding("utf8");
  proc.stderr.setEncoding("utf8");
  const emit = (text) => send("chat-data", { panelId, text });
  proc.stdout.on("data", (data) => {
    if (agent === "claude") return emit(data);
    buffered += data;
    let boundary;
    while ((boundary = buffered.indexOf("\n")) >= 0) {
      const line = buffered.slice(0, boundary);
      buffered = buffered.slice(boundary + 1);
      try {
        const event = JSON.parse(line);
        if (
          event.type === "item.completed" &&
          event.item?.type === "agent_message"
        )
          emit(event.item.text + "\n");
        else if (event.type === "error" || event.type === "turn.failed")
          errorOutput +=
            event.message || event.error?.message || "Agent request failed.";
      } catch {
        /* non-event diagnostic */
      }
    }
  });
  proc.stderr.on("data", (data) => {
    errorOutput = (errorOutput + data).slice(-8000);
  });
  proc.on("error", (error) => {
    chats.delete(panelId);
    send("chat-data", { panelId, done: true, error: error.message });
  });
  proc.on("close", (code) => {
    chats.delete(panelId);
    send("chat-data", {
      panelId,
      done: true,
      error: code ? errorOutput || `Agent exited (${code}).` : undefined,
    });
  });
  proc.stdin.on("error", () => {});
  proc.stdin.end(prompt);
  return { started: true };
});
handle("catalog", async (kind) => {
  if (kind !== "skills") return [];
  const results = [];
  for (const base of [
    path.join(os.homedir(), ".codex/skills"),
    path.join(os.homedir(), ".agents/skills"),
  ]) {
    const entries = await fs
      .readdir(base, { withFileTypes: true })
      .catch(() => []);
    for (const entry of entries.filter((item) => !item.name.startsWith("."))) {
      const skillPath = path.join(base, entry.name, "SKILL.md");
      try {
        const source = await fs.readFile(skillPath, "utf8");
        results.push({
          name: entry.name,
          description:
            source.match(/^description:\s*(.+)$/m)?.[1] || "Local agent skill",
          path: skillPath,
        });
      } catch {
        /* not a skill */
      }
    }
  }
  return results;
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
  for (const pending of terminalPending.values()) pending.cancelled = true;
  for (const terminal of terminals.values())
    if (!terminal.exited) terminal.proc.kill();
  for (const panelId of chats.keys()) stopChat(panelId);
  Promise.resolve(connections?.close()).finally(() => {
    quitReady = true;
    app.quit();
  });
});
