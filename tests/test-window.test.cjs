const { test } = require("node:test");
const assert = require("node:assert/strict");
const { testWindow } = require("../electron/test-window.cjs");

test("hidden mode builds an off-screen window that still paints", () => {
  const mode = testWindow({ SUSHIAI_TEST_WINDOW: "hidden" });
  assert.equal(mode.hidden, true);
  assert.deepEqual(mode.windowOptions, {
    show: false,
    paintWhenInitiallyHidden: true,
    skipTaskbar: true,
    webPreferences: { backgroundThrottling: false },
  });
  assert.equal(mode.mascot, "none");
});

test("hidden mode keeps a mascot only when the evidence seam asks", () => {
  const env = { SUSHIAI_TEST_WINDOW: "hidden", SUSHIAI_TEST_MASCOT: "1" };
  assert.equal(testWindow(env).mascot, "hidden");
});

test("visible and unset modes keep today's window options", () => {
  for (const env of [{}, { SUSHIAI_TEST_WINDOW: "visible" }]) {
    const mode = testWindow(env);
    assert.equal(mode.hidden, false);
    assert.equal(mode.windowOptions.show, true);
    assert.equal(mode.windowOptions.webPreferences.backgroundThrottling, true);
    assert.equal(mode.windowOptions.skipTaskbar, undefined);
    assert.equal(mode.mascot, "visible");
  }
});
