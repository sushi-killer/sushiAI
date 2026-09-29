const path = require("node:path");

const MASCOT_TIMED_MS = 10000;
const MASCOT_ANSWERED_MS = 2500;
const MASCOT_MAX_NOTICES = 5;
const MAX_ANSWER_CHARS = 2000;
const FOCUSES = new Set(["question", "summary", "report"]);
// orchd task ids are v4 UUIDs (orchd/src/engine/mod.rs validate_task_id).
const TASK_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const RERUNNABLE = new Set(["failed", "stopped"]);

const WIDTH = 400;
const HEIGHT_INPUT = 340;
const HEIGHT_TIMED = 230;
const MARGIN = 12;
const MAX_HEIGHT_RATIO = 0.7;

function noticeId(notice) {
  if (notice.kind === "core-update") return "core-update";
  const at = notice.host ? `${notice.host}/` : "";
  if (notice.kind === "input") return `input:${at}${notice.taskId}`;
  return `${notice.kind}:${at}${notice.taskId}:${notice.body}`;
}

/** How long a notice stays: a needs-input one until the owner acts (null),
 * a done/failed one for MASCOT_TIMED_MS. */
function noticeLifetimeMs(notice) {
  return notice.kind === "input" || notice.kind === "core-update"
    ? null
    : MASCOT_TIMED_MS;
}

/** The queue the mascot shows, newest first. Actions:
 * add (input notices dedupe per task, replacing the older one whether open
 * or answered; done/failed dedupe by kind:taskId:body; capped; the dev-only
 * core-update notice is a single entry that stays until dismissed and survives clear), dismiss (by id), clear,
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
      return state.filter((item) => item.kind === "core-update");
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

/** The failed/stopped notice a Run again click may restart, or null. Only a
 * UUID-shaped task id with such a notice still queued is accepted. */
function rerunNotice(taskId, queue) {
  if (typeof taskId !== "string" || !TASK_ID.test(taskId)) return null;
  return (
    queue.find((item) => RERUNNABLE.has(item.kind) && item.taskId === taskId) ??
    null
  );
}

/** True while the primary display's work area covers the whole screen: macOS
 * hides the menu bar and Dock in a fullscreen space (a fullscreen app, a
 * Keynote/PowerPoint slideshow). `baseline` is the same reading at launch; a
 * menu bar that always auto-hides reads true then, and the signal is off. */
function presentingFrom(display, baseline) {
  if (baseline) return false;
  const { bounds, workArea } = display;
  return (
    workArea.x === bounds.x &&
    workArea.y === bounds.y &&
    workArea.width === bounds.width &&
    workArea.height === bounds.height
  );
}

/** Polls `presentingFrom` and reports each change.
 * ponytail: Electron exposes no cross-app "frontmost window is fullscreen" or
 * "screen is being shared" signal, and switching Spaces does not reliably
 * fire display-metrics-changed, so this polls the work area. Its ceiling: a
 * fullscreen app on a secondary display, screen sharing without fullscreen,
 * and an owner whose menu bar always auto-hides are not detected. */
function watchPresenting({ screen, onChange, intervalMs = 1500 }) {
  const read = () => screen.getPrimaryDisplay();
  const baseline = presentingFrom(read(), false);
  let presenting = false;
  const timer = setInterval(() => {
    const next = presentingFrom(read(), baseline);
    if (next === presenting) return;
    presenting = next;
    onChange(next);
  }, intervalMs);
  timer.unref?.();
  return () => clearInterval(timer);
}

/** Window height: room for the answer field only when a needs-input notice
 * is queued. */
function mascotHeight(queue) {
  return queue.some((item) => item.kind === "input")
    ? HEIGHT_INPUT
    : HEIGHT_TIMED;
}

/** Height follows the content the renderer reports, between the queue's
 * minimum and MAX_HEIGHT_RATIO of the work area; the minimum wins when the
 * work area is too small for the cap. */
function mascotBounds(workArea, queue, contentHeight) {
  const minimum = mascotHeight(queue);
  const wanted =
    typeof contentHeight === "number" &&
    Number.isFinite(contentHeight) &&
    contentHeight > 0
      ? contentHeight
      : 0;
  const cap = Math.floor(MAX_HEIGHT_RATIO * workArea.height);
  const height = Math.max(minimum, Math.min(wanted, cap));
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
  policy = "visible",
  devURL,
  getService,
  showMainWindow,
  send,
  restart,
}) {
  let win = null;
  let queue = [];
  let destroyed = false;
  let loaded = false;
  let timer = null;
  let contentHeight = 0;
  let presenting = false;

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
    win.setBounds(
      mascotBounds(screen.getPrimaryDisplay().workArea, queue, contentHeight),
    );
  }

  function publish() {
    if (!win || win.isDestroyed()) return;
    win.webContents.send("mascot-notices", queue);
    win.webContents.send("mascot-presenting", presenting);
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
    if (policy === "none") return;
    ensureWindow();
    if (!loaded) return;
    place();
    publish();
    if (policy !== "hidden" && !win.isVisible()) win.showInactive();
  }

  function ensureWindow() {
    if (win && !win.isDestroyed()) return;
    win = new BrowserWindow({
      ...mascotBounds(screen.getPrimaryDisplay().workArea, queue),
      show: false,
      frame: false,
      transparent: true,
      backgroundColor: "#00000000",
      ...(policy === "hidden" ? { paintWhenInitiallyHidden: true } : {}),
      hasShadow: false,
      resizable: false,
      movable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      focusable: policy !== "hidden",
      skipTaskbar: true,
      alwaysOnTop: true,
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

  ipcMain.on("mascot-resize", (event, height) => {
    if (!own(event) || typeof height !== "number" || !Number.isFinite(height))
      return;
    contentHeight = height;
    if (loaded) place();
  });

  handle("mascot-answer", async (taskId, text) => {
    if (typeof taskId !== "string" || taskId.length > 200)
      throw new Error("Invalid task.");
    const service = getService();
    if (!service) throw new Error("The orchestrator is not running.");
    // A remote task is answered on the host it runs on.
    const host = queue.find((item) => item.taskId === taskId)?.host;
    const task = await service.call("task.get", { id: taskId }, host);
    const answer = validateAnswer(taskId, text, task, queue);
    await service.call("task.answer", { id: taskId, answer }, host);
    dispatch({ type: "answered", taskId, now: Date.now() });
    return "Answered";
  });

  handle("mascot-land", async (taskId) => {
    if (typeof taskId !== "string" || taskId.length > 200)
      throw new Error("Invalid task.");
    const notice = queue.find(
      (item) => item.kind === "done" && item.taskId === taskId && item.canLand,
    );
    if (!notice) throw new Error("That notice is gone.");
    const service = getService();
    if (!service) throw new Error("The orchestrator is not running.");
    await service.call("task.land", { id: taskId }, notice.host);
    dispatch({ type: "dismiss", id: notice.id });
    return "Landing";
  });

  handle("mascot-open", (taskId, focus) => {
    if (typeof taskId !== "string" || !FOCUSES.has(focus))
      throw new Error("Invalid notice.");
    const matches = queue.filter((item) => item.taskId === taskId);
    const notice = matches.find((item) => item.focus === focus) || matches[0];
    if (!notice) throw new Error("That notice is gone.");
    showMainWindow();
    send("orchestrator-open", {
      taskId,
      repo: notice.repo,
      focus,
      ...(notice.host ? { host: notice.host } : {}),
    });
    if (notice.kind !== "input") dispatch({ type: "dismiss", id: notice.id });
  });

  handle("mascot-rerun", async (taskId) => {
    const notice = rerunNotice(taskId, queue);
    if (!notice) throw new Error("That notice is gone.");
    const service = getService();
    if (!service) throw new Error("The orchestrator is not running.");
    await service.call("task.start", { id: notice.taskId });
    dispatch({ type: "dismiss", id: notice.id });
    return "Running";
  });

  handle("mascot-inbox", () => {
    if (!queue.length) throw new Error("No notices are queued.");
    showMainWindow();
    send("open-inbox");
  });

  // Asked for by the page after ⌥Space expands it, so the answer field can
  // take typing; never on a plain notice, which must not steal focus.
  handle("mascot-focus", () => {
    if (policy === "visible" && win.isVisible()) win.focus();
  });

  handle("mascot-dismiss", (id) => {
    if (typeof id !== "string") throw new Error("Invalid notice.");
    dispatch({ type: "dismiss", id });
  });

  handle("mascot-restart", () => {
    if (!queue.some((item) => item.kind === "core-update"))
      throw new Error("No core update is pending.");
    restart();
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
    /** ⌥Space: the page flips between the bubble stack and the pill. */
    toggle() {
      if (!queue.length || !loaded || !win || win.isDestroyed()) return;
      win.webContents.send("mascot-toggle");
    },
    /** A fullscreen app or a slideshow started (true) or ended (false). */
    setPresenting(value) {
      presenting = Boolean(value);
      publish();
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
  rerunNotice,
  presentingFrom,
  watchPresenting,
  MAX_HEIGHT_RATIO,
  mascotBounds,
  registerMascot,
};
