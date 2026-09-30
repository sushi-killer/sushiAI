const fs = require("node:fs");

/** Exit code main uses to ask scripts/dev.mjs for a fresh Electron. */
const DEV_RESTART_EXIT_CODE = 75;

/** Main/preload sources whose edits need an Electron restart to take effect. */
function isCoreSource(file) {
  return /\.(cjs|mjs|js|py)$/.test(file || "");
}

function exitAction(code) {
  return code === DEV_RESTART_EXIT_CODE ? "respawn" : "exit";
}

// ponytail: fs.watch recursive, debounce 250 ms; add chokidar only if macOS coalescing misses edits
/** Calls onChange(file) once edits to core sources under `dir` settle;
 * returns close(). */
function watchCore({ dir, onChange, debounceMs = 250 }) {
  let timer = null;
  const watcher = fs.watch(dir, { recursive: true }, (_, file) => {
    if (!isCoreSource(file)) return;
    clearTimeout(timer);
    timer = setTimeout(() => onChange(file), debounceMs);
  });
  return () => {
    clearTimeout(timer);
    watcher.close();
  };
}

module.exports = {
  DEV_RESTART_EXIT_CODE,
  isCoreSource,
  exitAction,
  watchCore,
};
