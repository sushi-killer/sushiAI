const test = require("node:test");
const assert = require("node:assert/strict");
const {
  ACCELERATOR,
  createMascotShortcut,
} = require("../electron/mascot-shortcut.cjs");
const { DEFAULT_PREFERENCES } = require("../electron/attention.cjs");

function fakeShortcuts({ taken = false, throws = false } = {}) {
  const active = new Map();
  return {
    active,
    register(accelerator, callback) {
      if (throws) throw new Error("refused");
      if (taken) return false;
      active.set(accelerator, callback);
      return true;
    },
    unregister(accelerator) {
      active.delete(accelerator);
    },
  };
}

test("the shortcut preference is off by default and nothing is registered", () => {
  assert.equal(DEFAULT_PREFERENCES.mascotShortcut, false);
  const shortcuts = fakeShortcuts();
  const shortcut = createMascotShortcut({
    globalShortcut: shortcuts,
    toggle: () => {},
  });
  shortcut.sync(DEFAULT_PREFERENCES.mascotShortcut);
  assert.equal(shortcuts.active.size, 0);
  assert.deepEqual(shortcut.status(), {
    accelerator: ACCELERATOR,
    registered: false,
    failed: false,
  });
});

test("turning it on registers, the key toggles the mascot, off unregisters", () => {
  const shortcuts = fakeShortcuts();
  let toggled = 0;
  const shortcut = createMascotShortcut({
    globalShortcut: shortcuts,
    toggle: () => toggled++,
  });
  assert.equal(shortcut.sync(true).registered, true);
  shortcuts.active.get(ACCELERATOR)();
  assert.equal(toggled, 1);
  shortcut.sync(true);
  assert.equal(shortcuts.active.size, 1);
  assert.equal(shortcut.sync(false).registered, false);
  assert.equal(shortcuts.active.size, 0);
});

test("close unregisters a live shortcut", () => {
  const shortcuts = fakeShortcuts();
  const shortcut = createMascotShortcut({
    globalShortcut: shortcuts,
    toggle: () => {},
  });
  shortcut.sync(true);
  shortcut.close();
  assert.equal(shortcuts.active.size, 0);
});

test("a taken or refused accelerator is reported as failed, then clears when turned off", () => {
  for (const options of [{ taken: true }, { throws: true }]) {
    const shortcuts = fakeShortcuts(options);
    const shortcut = createMascotShortcut({
      globalShortcut: shortcuts,
      toggle: () => {},
    });
    assert.deepEqual(shortcut.sync(true), {
      accelerator: ACCELERATOR,
      registered: false,
      failed: true,
    });
    assert.equal(shortcut.sync(false).failed, false);
  }
});
