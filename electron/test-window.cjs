/** Test-window mode: SUSHIAI_TEST_WINDOW=hidden keeps a test-launched app off
 * the owner's screen (no window, focus, dock icon, tray or mascot) while it
 * still paints, so screenshots and measurements stay correct. */
function testWindow(env = process.env) {
  const hidden = env.SUSHIAI_TEST_WINDOW === "hidden";
  return {
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

module.exports = { testWindow };
