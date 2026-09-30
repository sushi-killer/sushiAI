const ACCELERATOR = "Alt+Space";

/** Owns the optional global shortcut that toggles the mascot. `sync` is
 * idempotent: it registers or unregisters to match the preference and
 * remembers whether the OS refused the accelerator. */
function createMascotShortcut({ globalShortcut, toggle }) {
  let registered = false;
  let failed = false;

  function unregister() {
    if (registered) globalShortcut.unregister(ACCELERATOR);
    registered = false;
  }

  function sync(enabled) {
    if (!enabled) {
      unregister();
      failed = false;
      return status();
    }
    if (registered) return status();
    try {
      // Returns false when another app already holds the shortcut.
      registered =
        globalShortcut.register(ACCELERATOR, () => toggle()) === true;
    } catch {
      registered = false;
    }
    failed = !registered;
    return status();
  }

  function status() {
    return { accelerator: ACCELERATOR, registered, failed };
  }

  return { sync, status, close: unregister };
}

module.exports = { ACCELERATOR, createMascotShortcut };
