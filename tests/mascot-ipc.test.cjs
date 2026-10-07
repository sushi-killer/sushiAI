const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  registerMascot,
  presentingFrom,
  watchPresenting,
} = require("../electron/mascot.cjs");

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
  const outcome = {};
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
    act: async (source, key, actionId, text) => {
      calls.push(["act", source, key, actionId, text]);
      if (outcome.fail) throw new Error("source refused");
      return outcome.message;
    },
    showMainWindow: () => calls.push(["show"]),
    send: (channel, value) => calls.push(["send", channel, value]),
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
    mascot.add({ source: "src", title: "t", body: "b", ...notice });
    win()?.finishLoad();
  };
  return { mascot, handlers, calls, outcome, win, invoke, add };
}

const OPEN = [{ id: "open", label: "Open" }];

test("an action reaches its source with the notice's source and key", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "failed", key: "k1", actions: OPEN });
  await invoke("mascot-act", "src:k1", "open");
  assert.deepEqual(calls, [["act", "src", "k1", "open", undefined]]);
  mascot.destroy();
});

test("an action with no message dismisses the notice, one with a message confirms it", async () => {
  const quiet = setup();
  quiet.add({ kind: "done", key: "k1", actions: OPEN });
  assert.equal(await quiet.invoke("mascot-act", "src:k1", "open"), undefined);
  assert.equal(quiet.mascot.snapshot().length, 0);
  quiet.mascot.destroy();

  const said = setup();
  said.outcome.message = "Sent";
  said.add({ kind: "input", key: "k2", reply: true });
  assert.equal(
    await said.invoke("mascot-act", "src:k2", "reply", " hi "),
    "Sent",
  );
  assert.deepEqual(said.calls, [["act", "src", "k2", "reply", "hi"]]);
  const [entry] = said.mascot.snapshot();
  assert.equal(entry.confirmed, "Sent");
  assert.ok(entry.expiresAt > Date.now());
  said.mascot.destroy();
});

test("an action on an input notice that returns nothing keeps it queued", async () => {
  const { mascot, invoke, add } = setup();
  add({ kind: "input", key: "k1", reply: true, actions: OPEN });
  await invoke("mascot-act", "src:k1", "open");
  assert.equal(mascot.snapshot().length, 1);
  mascot.destroy();
});

test("a source error reaches the page and leaves the notice queued", async () => {
  const { mascot, outcome, invoke, add } = setup();
  outcome.fail = true;
  add({ kind: "done", key: "k1", actions: OPEN });
  await assert.rejects(
    invoke("mascot-act", "src:k1", "open"),
    /source refused/,
  );
  assert.equal(mascot.snapshot().length, 1);
  mascot.destroy();
});

test("an action is refused unless the notice is queued and offers it", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "done", key: "k1", actions: OPEN });
  for (const [id, action, text] of [
    ["src:missing", "open"],
    ["src:k1", "nope"],
    ["src:k1", "reply", "text"],
    ["../../etc/passwd", "open"],
    [42, "open"],
    [{ id: "src:k1" }, "open"],
    [undefined, "open"],
    ["src:k1", "open", { text: 1 }],
  ])
    await assert.rejects(invoke("mascot-act", id, action, text));
  assert.deepEqual(calls, []);
  mascot.destroy();
});

test("retract and clear remove a source's notices from the page", () => {
  const { mascot, add } = setup();
  add({ kind: "done", key: "k1" });
  add({ kind: "done", key: "k2" });
  add({ kind: "done", key: "k3", source: "other" });
  mascot.retract("src", "k1");
  assert.deepEqual(
    mascot.snapshot().map((item) => item.id),
    ["other:k3", "src:k2"],
  );
  mascot.clear("src");
  assert.deepEqual(
    mascot.snapshot().map((item) => item.id),
    ["other:k3"],
  );
  mascot.clear();
  assert.equal(mascot.snapshot().length, 0);
  mascot.destroy();
});

test("Answer all in Inbox shows the main window and asks it for the Inbox", async () => {
  const { mascot, calls, invoke, add } = setup();
  add({ kind: "input", key: "k1" });
  await invoke("mascot-inbox", "ignored", { extra: true });
  assert.deepEqual(calls, [["show"], ["send", "open-inbox", undefined]]);
  mascot.destroy();
});

test("mascot IPC rejects a sender that is not the mascot page", async () => {
  const { mascot, handlers, calls, add } = setup();
  add({ kind: "failed", key: "k1", actions: OPEN });
  const stranger = { sender: {}, senderFrame: {} };
  for (const channel of ["mascot-act", "mascot-inbox", "mascot-focus"])
    await assert.rejects(
      handlers.get(channel)(stranger, "src:k1", "open"),
      /Untrusted IPC sender/,
    );
  assert.deepEqual(calls, []);
  mascot.destroy();
});

test("toggle and presenting reach the page; focus only for a visible mascot", async () => {
  const { mascot, win, invoke, add } = setup();
  mascot.toggle();
  add({ kind: "input", key: "k1" });
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
  hidden.add({ kind: "input", key: "k1" });
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
