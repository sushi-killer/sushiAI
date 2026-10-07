const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  createNotices,
  validateNotice,
} = require("../electron/extensions/notices.cjs");

class FakeNotification {
  static shown = [];
  constructor(options) {
    this.options = options;
    this.handlers = {};
    this.closed = false;
  }
  on(name, callback) {
    this.handlers[name] = callback;
  }
  show() {
    FakeNotification.shown.push(this);
  }
  close() {
    this.closed = true;
    this.handlers.close?.();
  }
}

function setup(prefs = { notifications: true, desktopMascot: true }) {
  FakeNotification.shown = [];
  const log = [];
  const preferences = { ...prefs };
  const notices = createNotices({
    preferences: () => preferences,
    mascot: {
      add: (notice) => log.push(["add", notice]),
      retract: (source, key) => log.push(["retract", source, key]),
      clear: (source) => log.push(["clear", source]),
    },
    showWindow: () => log.push(["show"]),
    Notification: FakeNotification,
    icon: "icon",
  });
  return { notices, log, preferences };
}

const notice = (key, extra = {}) => ({
  key,
  kind: "done",
  title: "Title",
  body: "Body",
  ...extra,
});

test("a notice goes nowhere when notifications are off", () => {
  for (const desktopMascot of [true, false]) {
    const { notices, log } = setup({ notifications: false, desktopMascot });
    assert.equal(notices.publish("s", notice("a")), "none");
    assert.deepEqual(log, []);
    assert.equal(FakeNotification.shown.length, 0);
  }
});

test("a notice is queued in the mascot, tagged with its source, when the mascot is on", () => {
  const { notices, log } = setup();
  assert.equal(
    notices.publish("s", notice("a", { choices: ["x"], reply: true })),
    "mascot",
  );
  assert.equal(log.length, 1);
  assert.equal(log[0][0], "add");
  assert.equal(log[0][1].source, "s");
  assert.equal(log[0][1].key, "a");
  assert.deepEqual(log[0][1].choices, ["x"]);
  assert.equal(log[0][1].reply, true);
  assert.equal(FakeNotification.shown.length, 0);
});

test("a notice is native when the mascot is off, and a click opens it at its source", async () => {
  const { notices, log } = setup({ notifications: true, desktopMascot: false });
  const seen = [];
  notices.register("s", async (key, actionId, text) => {
    seen.push([key, actionId, text]);
  });
  assert.equal(notices.publish("s", notice("a")), "native");
  const [native] = FakeNotification.shown;
  assert.deepEqual(native.options, {
    title: "Title",
    body: "Body",
    icon: "icon",
  });
  native.handlers.click();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(log, [["show"]]);
  assert.deepEqual(seen, [["a", "open", undefined]]);
});

test("a native click for a source without a handler or with a failing handler does not throw", async () => {
  const { notices } = setup({ notifications: true, desktopMascot: false });
  notices.publish("none", notice("a"));
  FakeNotification.shown[0].handlers.click();
  notices.register("bad", async () => {
    throw new Error("nope");
  });
  notices.publish("bad", notice("a"));
  FakeNotification.shown[1].handlers.click();
  await new Promise((resolve) => setImmediate(resolve));
});

test("a native notice replaces the older one with the same source and key", () => {
  const { notices } = setup({ notifications: true, desktopMascot: false });
  notices.publish("s", notice("a"));
  notices.publish("s", notice("b"));
  notices.publish("s", notice("a", { body: "newer" }));
  const [first, second, third] = FakeNotification.shown;
  assert.equal(first.closed, true);
  assert.equal(second.closed, false);
  assert.equal(third.closed, false);
  assert.equal(third.options.body, "newer");
});

test("retract and clear remove a notice wherever it is shown", () => {
  const { notices, log } = setup({ notifications: true, desktopMascot: false });
  notices.publish("s", notice("a"));
  notices.publish("s", notice("b"));
  notices.publish("t", notice("a"));
  notices.retract("s", "a");
  assert.deepEqual(
    FakeNotification.shown.map((item) => item.closed),
    [true, false, false],
  );
  notices.clear("s");
  assert.deepEqual(
    FakeNotification.shown.map((item) => item.closed),
    [true, true, false],
  );
  assert.deepEqual(log, [
    ["retract", "s", "a"],
    ["clear", "s"],
  ]);
});

test("a source sees only its own actions, and an unknown source is refused", async () => {
  const { notices } = setup();
  const seen = [];
  notices.register("s", async (...args) => {
    seen.push(args);
    return "Done";
  });
  assert.equal(await notices.act("s", "a", "go", "text"), "Done");
  assert.deepEqual(seen, [["a", "go", "text"]]);
  await assert.rejects(notices.act("other", "a", "go"), /gone/);
});

test("the mascot preference is read when a notice is published", () => {
  const { notices, log, preferences } = setup();
  notices.publish("s", notice("a"));
  preferences.desktopMascot = false;
  notices.publish("s", notice("b"));
  assert.equal(log.filter(([name]) => name === "add").length, 1);
  assert.equal(FakeNotification.shown.length, 1);
});

test("validateNotice rejects a malformed notice and copies a good one", () => {
  const bad = (value) =>
    assert.throws(() => validateNotice(value), /Invalid|Duplicate/);
  bad(null);
  bad([]);
  bad(notice(""));
  bad(notice("a", { kind: "other" }));
  bad(notice("a", { title: "" }));
  bad(notice("a", { title: "x".repeat(121) }));
  bad(notice("a", { body: "x".repeat(2001) }));
  bad(notice("a", { choices: "x" }));
  bad(notice("a", { choices: Array(9).fill("x") }));
  bad(notice("a", { meta: [1] }));
  bad(notice("a", { at: -1 }));
  bad(notice("a", { actions: [{ id: "Bad Id", label: "x" }] }));
  bad(notice("a", { actions: [{ id: "ok", label: "" }] }));
  bad(notice("a", { actions: [{ id: "ok", label: "x", icon: "skull" }] }));
  bad(
    notice("a", {
      actions: [
        { id: "ok", label: "x" },
        { id: "ok", label: "y" },
      ],
    }),
  );
  bad(notice("a", { actions: Array(5).fill({ id: "ok", label: "x" }) }));
  const input = notice("a", {
    extra: "dropped",
    sticky: true,
    reply: true,
    actions: [{ id: "open", label: "Open", emphasis: "ghost", icon: "check" }],
  });
  const clean = validateNotice(input);
  assert.equal(clean.extra, undefined);
  assert.equal(clean.sticky, true);
  assert.equal(clean.reply, true);
  assert.deepEqual(clean.actions, input.actions);
  assert.notEqual(clean.actions[0], input.actions[0]);
});
