const { test } = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const {
  testWindow,
  isolateTestProfile,
} = require("../electron/test-window.cjs");

function fakeApp(userData) {
  const calls = { switches: [], paths: [] };
  return {
    calls,
    commandLine: {
      appendSwitch: (...args) => calls.switches.push(args),
    },
    getPath: (name) => {
      assert.equal(name, "userData");
      return userData;
    },
    setPath: (...args) => calls.paths.push(args),
  };
}

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

test("test reports every SUSHIAI_TEST_WINDOW launch and not an unset one", () => {
  assert.equal(testWindow({ SUSHIAI_TEST_WINDOW: "hidden" }).test, true);
  assert.equal(testWindow({ SUSHIAI_TEST_WINDOW: "visible" }).test, true);
  assert.equal(testWindow({}).test, false);
});

test("hidden test mode isolates the app profile", () => {
  const userData = "/tmp/sushiai-hidden-profile";
  const app = fakeApp(userData);
  const created = [];

  isolateTestProfile(
    app,
    testWindow({ SUSHIAI_TEST_WINDOW: "hidden" }),
    (dir) => created.push(dir),
  );

  assert.deepEqual(app.calls.switches, [["use-mock-keychain"]]);
  assert.deepEqual(app.calls.paths, [
    ["desktop", path.join(userData, "desktop")],
    ["documents", path.join(userData, "documents")],
    ["downloads", path.join(userData, "downloads")],
  ]);
  assert.deepEqual(
    created,
    app.calls.paths.map(([, dir]) => dir),
  );
});

test("visible test mode isolates the app profile", () => {
  const userData = "/tmp/sushiai-visible-profile";
  const app = fakeApp(userData);
  const created = [];

  isolateTestProfile(
    app,
    testWindow({ SUSHIAI_TEST_WINDOW: "visible" }),
    (dir) => created.push(dir),
  );

  assert.deepEqual(app.calls.switches, [["use-mock-keychain"]]);
  assert.deepEqual(app.calls.paths, [
    ["desktop", path.join(userData, "desktop")],
    ["documents", path.join(userData, "documents")],
    ["downloads", path.join(userData, "downloads")],
  ]);
  assert.deepEqual(
    created,
    app.calls.paths.map(([, dir]) => dir),
  );
});

test("unset test mode leaves keychain and folder paths untouched", () => {
  const app = fakeApp("/tmp/sushiai-normal-profile");
  const created = [];

  isolateTestProfile(app, testWindow({}), (dir) => created.push(dir));

  assert.deepEqual(app.calls.switches, []);
  assert.deepEqual(app.calls.paths, []);
  assert.deepEqual(created, []);
});
