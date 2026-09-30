const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  registerMascot,
  rerunNotice,
  presentingFrom,
  watchPresenting,
} = require("../electron/mascot.cjs");

const TASK = "0f8fad5b-d9cb-469f-a165-70867728950e";

class FakeWindow {
  constructor() {
    this.sent = [];
    this.focused = 0;
    this.visible = false;
    this.destroyed = false;
    const loaders = [];
    this.webContents = {
      mainFrame: {},
      send: (channel, value) => this.sent.push([channel, value]),
      on: () => {},
      once: (_name, callback) => loaders.push(callback),
      setWindowOpenHandler: () => {},
    };
    this.finishLoad = () => loaders.splice(0).forEach((fn) => fn());
  }
  setAlwaysOnTop() {}
  setVisibleOnAllWorkspaces() {}
  on() {}
  loadFile() {}
  loadURL() {}
  setBounds() {}
  isDestroyed() {
    return this.destroyed;
  }
  isVisible() {
    return this.visible;
  }
  showInactive() {
    this.visible = true;
  }
  hide() {
    this.visible = false;
  }
  focus() {
    this.focused += 1;
  }
  destroy() {
    this.destroyed = true;
  }
}

function setup(policy = "visible") {
  const handlers = new Map();
  const calls = [];
  const windows = [];
  const mascot = registerMascot({
    ipcMain: {
      handle: (channel, callback) => handlers.set(channel, callback),
      on: () => {},
    },
    BrowserWindow: function BrowserWindow() {
      const win = new FakeWindow();
      windows.push(win);
      return win;
    },
    screen: {
      getPrimaryDisplay: () => ({
        workArea: { x: 0, y: 25, width: 1440, height: 875 },
      }),
    },
    root: "/app",
    policy,
    getService: () => ({
      call: async (method, params, host) => {
        calls.push(host ? [method, params, host] : [method, params]);
        return {};
      },
    }),
    showMainWindow: () => calls.push(["show"]),
    send: (channel, value) => calls.push(["send", channel, value]),
    restart: () => {},
  });
  const win = () => windows[0];
  const invoke = (channel, ...args) => {
    const event = {
      sender: win().webContents,
      senderFrame: win().webContents.mainFrame,
    };
    return handlers.get(channel)(event, ...args);
  };
  const add = (notice) => {
    mascot.add({ repo: "/repo", title: "t", body: "b", ...notice });
    win()?.finishLoad();
  };
  return { mascot, handlers, calls, win, invoke, add };
}

test("Run again starts a queued failed or stopped task and drops its notice", async () => {
  for (const kind of ["failed", "stopped"]) {
    const { mascot, calls, invoke, add } = setup();
    add({ kind, taskId: TASK, focus: "summary" });
    assert.equal(await invoke("mascot-rerun", TASK), "Running");
    assert.deepEqual(calls, [["task.start", { id: TASK }]]);
    assert.equal(mascot.snapshot().length, 0);
    mascot.destroy();
  }
});

test("Run again starts a remote task on the host it runs on", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "failed", taskId: TASK, focus: "summary", host: "ssh:box" });
  assert.equal(await invoke("mascot-rerun", TASK), "Running");
  assert.deepEqual(calls, [["task.start", { id: TASK }, "ssh:box"]]);
  mascot.destroy();
});

test("Run again refuses anything but a queued failed/stopped task id", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "done", taskId: TASK, focus: "summary" });
  for (const bad of [
    TASK, // only a done notice is queued for it
    "../../etc/passwd",
    "a",
    `${TASK} `,
    42,
    { id: TASK },
    undefined,
  ])
    await assert.rejects(invoke("mascot-rerun", bad), /That notice is gone/);
  assert.deepEqual(calls, []);
  mascot.destroy();
});

test("rerunNotice checks the id shape before the queue", () => {
  const queue = [{ id: "x", kind: "failed", taskId: "not-a-uuid" }];
  assert.equal(rerunNotice("not-a-uuid", queue), null);
  const ok = [{ id: "y", kind: "stopped", taskId: TASK }];
  assert.equal(rerunNotice(TASK, ok), ok[0]);
  assert.equal(rerunNotice(TASK.toUpperCase(), ok), null);
});

test("Answer all in Inbox shows the main window and asks it for the Inbox", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "input", taskId: TASK, focus: "question" });
  await invoke("mascot-inbox", "ignored", { extra: true });
  assert.deepEqual(calls, [["show"], ["send", "open-inbox", undefined]]);
  mascot.destroy();
});

test("mascot IPC rejects a sender that is not the mascot page", async () => {
  const { mascot, handlers, calls, add } = setup();
  add({ kind: "failed", taskId: TASK, focus: "summary" });
  const stranger = { sender: {}, senderFrame: {} };
  for (const channel of ["mascot-rerun", "mascot-inbox", "mascot-focus"])
    await assert.rejects(
      handlers.get(channel)(stranger, TASK),
      /Untrusted IPC sender/,
    );
  assert.deepEqual(calls, []);
  mascot.destroy();
});

test("toggle and presenting reach the page; focus only for a visible mascot", async () => {
  const { mascot, win, invoke, add } = setup();
  mascot.toggle();
  add({ kind: "input", taskId: TASK, focus: "question" });
  mascot.toggle();
  mascot.setPresenting(true);
  const channels = win().sent.map(([channel, value]) =>
    channel === "mascot-notices" ? channel : `${channel}:${value}`,
  );
  assert.ok(channels.includes("mascot-toggle:undefined"));
  assert.equal(channels.at(-1), "mascot-presenting:true");
  await invoke("mascot-focus");
  assert.equal(win().focused, 1);
  mascot.destroy();

  const hidden = setup("hidden");
  hidden.add({ kind: "input", taskId: TASK, focus: "question" });
  await hidden.invoke("mascot-focus");
  assert.equal(hidden.win().focused, 0);
  hidden.mascot.destroy();
});

const display = (workArea) => ({
  bounds: { x: 0, y: 0, width: 1440, height: 900 },
  workArea,
});
const full = { x: 0, y: 0, width: 1440, height: 900 };
const normal = { x: 0, y: 25, width: 1440, height: 800 };

test("presenting is a work area that covers the whole display", () => {
  assert.equal(presentingFrom(display(full), false), true);
  assert.equal(presentingFrom(display(normal), false), false);
  // A menu bar that always auto-hides read as presenting at launch: off.
  assert.equal(presentingFrom(display(full), true), false);
});

test("watchPresenting reports each change once and stops on close", async () => {
  let current = normal;
  const changes = [];
  const close = watchPresenting({
    screen: { getPrimaryDisplay: () => display(current) },
    onChange: (value) => changes.push(value),
    intervalMs: 5,
  });
  const tick = () => new Promise((resolve) => setTimeout(resolve, 25));
  await tick();
  current = full;
  await tick();
  await tick();
  current = normal;
  await tick();
  close();
  current = full;
  await tick();
  assert.deepEqual(changes, [true, false]);
});
