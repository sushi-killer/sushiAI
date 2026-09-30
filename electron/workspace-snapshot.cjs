// The workspace snapshot: one JSON file in the profile folder that owns
// what the renderer used to keep in localStorage. Chromium commits
// localStorage to disk on a delay and at a clean shutdown, so a killed app
// lost everything since the last commit; this file is written by us,
// atomically, and flushed synchronously when the window goes away.
const fs = require("node:fs");
const path = require("node:path");

const FILE = "workspace-state.json";
// A renderer bug must not fill the disk with one runaway write.
const MAX_BYTES = 64 * 1024 * 1024;

function snapshotFile(dir) {
  return path.join(dir, FILE);
}

/** The stored text, or null when there is no snapshot yet. */
function readSnapshot(dir) {
  try {
    return fs.readFileSync(snapshotFile(dir), "utf8");
  } catch {
    return null;
  }
}

/** Writes the whole file to a sibling and renames it over the target, so a
 * kill mid-write leaves the previous snapshot, never half of the new one. */
function writeSnapshotSync(dir, text) {
  if (typeof text !== "string" || Buffer.byteLength(text) > MAX_BYTES)
    throw new Error("Invalid workspace snapshot");
  fs.mkdirSync(dir, { recursive: true });
  const target = snapshotFile(dir);
  const tmp = `${target}.${process.pid}.tmp`;
  try {
    fs.writeFileSync(tmp, text);
    fs.renameSync(tmp, target);
  } catch (error) {
    fs.rmSync(tmp, { force: true });
    throw error;
  }
}

/** `handle` is main.cjs's sender-checked ipcMain.handle wrapper; the
 * synchronous channels go through `on` and get the same sender check. */
function registerWorkspaceSnapshot({
  ipcMain,
  handle,
  getMainWindow,
  userDataDir,
}) {
  const trusted = (event) => {
    const window = getMainWindow();
    return (
      Boolean(window) &&
      event.sender === window.webContents &&
      event.senderFrame === window.webContents.mainFrame
    );
  };
  // A flush overtakes any write still waiting behind it, so an older debounced
  // write can never land on top of the newer synchronous one.
  let version = 0;
  let queue = Promise.resolve();
  const sync = (channel, run) =>
    ipcMain.on(channel, (event, ...args) => {
      if (!trusted(event)) {
        event.returnValue = { error: "Untrusted IPC sender" };
        return;
      }
      try {
        event.returnValue = { value: run(...args) };
      } catch (error) {
        event.returnValue = { error: String(error?.message ?? error) };
      }
    });
  sync("workspace-state-read", () => readSnapshot(userDataDir()));
  sync("workspace-state-flush", (text) => {
    version += 1;
    writeSnapshotSync(userDataDir(), text);
    return null;
  });
  handle("workspace-state-write", (text) => {
    const mine = ++version;
    queue = queue
      .catch(() => {})
      .then(() => {
        if (mine === version) writeSnapshotSync(userDataDir(), text);
      });
    return queue;
  });
}

module.exports = {
  FILE,
  snapshotFile,
  readSnapshot,
  writeSnapshotSync,
  registerWorkspaceSnapshot,
};
