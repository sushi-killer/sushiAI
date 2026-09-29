const path = require("node:path");

const MASCOT_TIMED_MS = 10000;
const MASCOT_ANSWERED_MS = 2500;
const MASCOT_MAX_NOTICES = 5;
const MAX_ANSWER_CHARS = 2000;
const FOCUSES = new Set(["question", "summary", "report"]);

const WIDTH = 400;
const HEIGHT_INPUT = 340;
const HEIGHT_TIMED = 230;
const MARGIN = 12;

function noticeId(notice) {
  return `${notice.kind}:${notice.taskId}:${notice.body}`;
}

/** How long a notice stays: a needs-input one until the owner acts (null),
 * a done/failed one for MASCOT_TIMED_MS. */
function noticeLifetimeMs(notice) {
  return notice.kind === "input" ? null : MASCOT_TIMED_MS;
}

/** The queue the mascot shows, newest first. Actions:
 * add (deduped by kind:taskId:body, capped), dismiss (by id), clear,
 * task (a needs-input notice whose task left `waiting` is dropped),
 * answered (marks a task's input notices as answered and gives them a short
 * life), expire (drops every notice whose `expiresAt` has passed). */
function queueReducer(state, action) {
  switch (action.type) {
    case "add": {
      const id = noticeId(action.notice);
      const lifetime = noticeLifetimeMs(action.notice);
      const entry = {
        ...action.notice,
        id,
        expiresAt: lifetime === null ? null : action.now + lifetime,
      };
      return [entry, ...state.filter((item) => item.id !== id)].slice(
        0,
        MASCOT_MAX_NOTICES,
      );
    }
    case "dismiss":
      return state.filter((item) => item.id !== action.id);
    case "clear":
      return [];
    case "task":
      return action.status === "waiting"
        ? state
        : state.filter(
            (item) =>
              !(
                item.kind === "input" &&
                !item.answered &&
                item.taskId === action.taskId
              ),
          );
    case "answered":
      return state.map((item) =>
        item.kind === "input" && item.taskId === action.taskId
          ? {
              ...item,
              answered: true,
              expiresAt: action.now + MASCOT_ANSWERED_MS,
            }
          : item,
      );
    case "expire":
      return state.filter(
        (item) => item.expiresAt === null || item.expiresAt > action.now,
      );
    default:
      return state;
  }
}

/** Checks a quick answer before it reaches the daemon and returns the text to
 * send. The task must be waiting with a question and belong to a queued,
 * still-open needs-input notice; the text is trimmed, non-empty and capped. */
function validateAnswer(taskId, text, task, queue = []) {
  if (typeof taskId !== "string" || !taskId) throw new Error("Invalid task.");
  if (
    !queue.some(
      (item) =>
        item.kind === "input" && !item.answered && item.taskId === taskId,
    )
  )
    throw new Error("No open question for that task.");
  if (
    !task ||
    task.id !== taskId ||
    task.status !== "waiting" ||
    !task.question
  )
    throw new Error("That task is not waiting for an answer.");
  if (typeof text !== "string") throw new Error("Invalid answer.");
  const answer = text.trim();
  if (!answer) throw new Error("Type an answer first.");
  if (answer.length > MAX_ANSWER_CHARS)
    throw new Error("That answer is too long.");
  return answer;
}

/** Window height: room for the answer field only when a needs-input notice
 * is queued. */
function mascotHeight(queue) {
  return queue.some((item) => item.kind === "input")
    ? HEIGHT_INPUT
    : HEIGHT_TIMED;
}

function mascotBounds(workArea, queue) {
  const height = mascotHeight(queue);
  return {
    width: WIDTH,
    height,
    x: workArea.x + workArea.width - WIDTH - MARGIN,
    y: workArea.y + workArea.height - height - MARGIN,
  };
}

/** Owns the mascot window and its queue. `add`/`clear`/`onTask` are called by
 * the notice path; `destroy` when the main window closes or the app quits. */
function registerMascot({
  ipcMain,
  BrowserWindow,
  screen,
  root,
  devURL,
  getService,
  showMainWindow,
  send,
}) {
  let win = null;
  let queue = [];
  let destroyed = false;
  let loaded = false;
  let timer = null;

  const own = (event) =>
    Boolean(win) &&
    !win.isDestroyed() &&
    event.sender === win.webContents &&
    event.senderFrame === win.webContents.mainFrame;

  function handle(channel, callback) {
    ipcMain.handle(channel, async (event, ...args) => {
      if (!own(event)) throw new Error("Untrusted IPC sender");
      return callback(...args);
    });
  }

  function place() {
    win.setBounds(mascotBounds(screen.getPrimaryDisplay().workArea, queue));
  }

  function publish() {
    if (!win || win.isDestroyed()) return;
    win.webContents.send("mascot-notices", queue);
  }

  function schedule() {
    clearTimeout(timer);
    timer = null;
    const times = queue.map((item) => item.expiresAt).filter((t) => t !== null);
    if (!times.length) return;
    const delay = Math.max(0, Math.min(...times) - Date.now());
    timer = setTimeout(
      () => dispatch({ type: "expire", now: Date.now() }),
      delay,
    );
    timer.unref?.();
  }

  function dispatch(action) {
    const next = queueReducer(queue, action);
    if (
      next.length === queue.length &&
      next.every((item, index) => item === queue[index])
    )
      return;
    queue = next;
    schedule();
    sync();
  }

  function sync() {
    if (destroyed) return;
    if (!queue.length) {
      publish();
      if (win && !win.isDestroyed()) win.hide();
      return;
    }
    ensureWindow();
    if (!loaded) return;
    place();
    publish();
    if (!win.isVisible()) win.showInactive();
  }

  function ensureWindow() {
    if (win && !win.isDestroyed()) return;
    win = new BrowserWindow({
      ...mascotBounds(screen.getPrimaryDisplay().workArea, queue),
      show: false,
      frame: false,
      transparent: true,
      backgroundColor: "#00000000",
      hasShadow: false,
      resizable: false,
      movable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      skipTaskbar: true,
      alwaysOnTop: true,
      focusable: true,
      title: "sushiAI mascot",
      webPreferences: {
        preload: path.join(__dirname, "mascot-preload.cjs"),
        contextIsolation: true,
        sandbox: true,
        nodeIntegration: false,
      },
    });
    win.setAlwaysOnTop(true, "screen-saver");
    win.setVisibleOnAllWorkspaces(true, {
      visibleOnFullScreen: true,
      skipTransformProcessType: true,
    });
    win.webContents.on("will-navigate", (event) => event.preventDefault());
    win.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    loaded = false;
    win.webContents.once("did-finish-load", () => {
      loaded = true;
      sync();
    });
    win.on("closed", () => {
      win = null;
    });
    if (devURL === "http://127.0.0.1:5173")
      win.loadURL(`${devURL}/mascot.html`);
    else win.loadFile(path.join(root, "dist/mascot.html"));
  }

  ipcMain.on("mascot-sync", (event) => {
    if (own(event)) publish();
  });

  handle("mascot-answer", async (taskId, text) => {
    if (typeof taskId !== "string" || taskId.length > 200)
      throw new Error("Invalid task.");
    const service = getService();
    if (!service) throw new Error("The orchestrator is not running.");
    const task = await service.call("task.get", { id: taskId });
    const answer = validateAnswer(taskId, text, task, queue);
    await service.call("task.answer", { id: taskId, answer });
    dispatch({ type: "answered", taskId, now: Date.now() });
    return "Answered";
  });

  handle("mascot-open", (taskId, focus) => {
    if (typeof taskId !== "string" || !FOCUSES.has(focus))
      throw new Error("Invalid notice.");
    const matches = queue.filter((item) => item.taskId === taskId);
    const notice = matches.find((item) => item.focus === focus) || matches[0];
    if (!notice) throw new Error("That notice is gone.");
    showMainWindow();
    send("orchestrator-open", { taskId, repo: notice.repo, focus });
    if (notice.kind !== "input") dispatch({ type: "dismiss", id: notice.id });
  });

  handle("mascot-dismiss", (id) => {
    if (typeof id !== "string") throw new Error("Invalid notice.");
    dispatch({ type: "dismiss", id });
  });

  return {
    add(notice) {
      dispatch({ type: "add", notice, now: Date.now() });
    },
    clear() {
      dispatch({ type: "clear" });
    },
    onTask(task) {
      dispatch({ type: "task", taskId: task.id, status: task.status });
    },
    snapshot: () => queue,
    getWindow: () => (win && !win.isDestroyed() ? win : null),
    destroy() {
      destroyed = true;
      clearTimeout(timer);
      queue = [];
      if (win && !win.isDestroyed()) win.destroy();
      win = null;
    },
  };
}

module.exports = {
  MASCOT_TIMED_MS,
  MASCOT_ANSWERED_MS,
  MASCOT_MAX_NOTICES,
  MAX_ANSWER_CHARS,
  noticeId,
  noticeLifetimeMs,
  queueReducer,
  validateAnswer,
  mascotBounds,
  registerMascot,
};
