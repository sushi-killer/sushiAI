const fs = require("node:fs");
const path = require("node:path");

/** Test-window mode: SUSHIAI_TEST_WINDOW=hidden keeps a test-launched app off
 * the owner's screen (no window, focus, dock icon, tray or mascot) while it
 * still paints, so screenshots and measurements stay correct. */
function testWindow(env = process.env) {
  const hidden = env.SUSHIAI_TEST_WINDOW === "hidden";
  return {
    test: Boolean(env.SUSHIAI_TEST_WINDOW),
    hidden,
    mascot: hidden
      ? env.SUSHIAI_TEST_MASCOT === "1"
        ? "hidden"
        : "none"
      : "visible",
    windowOptions: hidden
      ? {
          show: false,
          paintWhenInitiallyHidden: true,
          skipTaskbar: true,
          webPreferences: { backgroundThrottling: false },
        }
      : { show: true, webPreferences: { backgroundThrottling: true } },
  };
}

function isolateTestProfile(
  app,
  mode,
  mkdir = (dir) => fs.mkdirSync(dir, { recursive: true }),
) {
  if (!mode.test) return;

  app.commandLine.appendSwitch("use-mock-keychain");
  const userData = app.getPath("userData");
  for (const name of ["desktop", "documents", "downloads"]) {
    const dir = path.join(userData, name);
    mkdir(dir);
    app.setPath(name, dir);
  }
}

module.exports = { testWindow, isolateTestProfile };
