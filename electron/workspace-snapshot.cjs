// The workspace snapshot: what the renderer used to keep in localStorage,
// owned by us in `sushiai.db` so it survives a killed app. The renderer still
// hands over one JSON text; it is split into `workspaces` rows and `app_state`
// keys and only what changed is written, in one transaction.
const { appDb, transaction } = require("./app-db.cjs");

// A renderer bug must not fill the disk with one runaway write.
const MAX_BYTES = 64 * 1024 * 1024;

const isObject = (value) =>
  Boolean(value) && typeof value === "object" && !Array.isArray(value);

/** Parses stored JSON; undefined (with a warning) for an unreadable row. */
function parseRow(text, what) {
  try {
    return JSON.parse(text);
  } catch (error) {
    console.warn(`Skipping an unreadable ${what}: ${error.message}`);
    return undefined;
  }
}

/** The stored text, or null when there is no snapshot yet. */
function readSnapshot(dir) {
  const db = appDb(dir);
  const state = db.prepare("SELECT key, value FROM app_state").all();
  const rows = db
    .prepare("SELECT data FROM workspaces ORDER BY position")
    .all();
  if (!state.length && !rows.length) return null;
  const snapshot = {};
  for (const { key, value } of state) {
    const parsed = parseRow(value, `state key ${key}`);
    if (parsed !== undefined) snapshot[key] = parsed;
  }
  snapshot.workspaces = rows
    .map((row) => parseRow(row.data, "workspace"))
    .filter((workspace) => workspace !== undefined);
  return JSON.stringify(snapshot);
}

/** The saved workspaces in sidebar order. */
function savedWorkspaces(dir) {
  return appDb(dir)
    .prepare("SELECT data FROM workspaces ORDER BY position")
    .all()
    .map((row) => parseRow(row.data, "workspace"))
    .filter((workspace) => workspace !== undefined);
}

/** The workspaces to store, keyed by id. A repeated id keeps the first row;
 * the endpoint is kept beside the data for lookups. */
function uniqueWorkspaces(workspaces) {
  const ids = new Set();
  const kept = [];
  for (const workspace of workspaces) {
    if (!isObject(workspace) || workspace.id == null || workspace.id === "")
      continue;
    const id = String(workspace.id);
    if (ids.has(id)) continue;
    ids.add(id);
    kept.push({
      id,
      endpoint:
        typeof workspace.connection === "string" ? workspace.connection : "",
      data: JSON.stringify(workspace),
    });
  }
  return kept;
}

/** Writes `snapshot` (a plain object) as row deltas. Runs inside a
 * transaction opened by the caller. */
function applyDelta(db, snapshot) {
  const { workspaces, ...state } = snapshot;
  if (workspaces !== undefined && !Array.isArray(workspaces))
    throw new Error("Invalid workspace snapshot");

  const storedState = new Map(
    db
      .prepare("SELECT key, value FROM app_state")
      .all()
      .map((row) => [row.key, row.value]),
  );
  const upsertState = db.prepare(
    "INSERT INTO app_state(key, value) VALUES(?, ?) " +
      "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
  );
  const deleteState = db.prepare("DELETE FROM app_state WHERE key = ?");
  for (const [key, value] of Object.entries(state)) {
    const text = JSON.stringify(value);
    if (text === undefined) continue;
    if (storedState.get(key) !== text) upsertState.run(key, text);
    storedState.delete(key);
  }
  for (const key of storedState.keys()) deleteState.run(key);

  const next = uniqueWorkspaces(workspaces ?? []);
  const stored = new Map(
    db
      .prepare("SELECT id, position, endpoint, data FROM workspaces")
      .all()
      .map((row) => [row.id, row]),
  );
  const nextIds = new Set(next.map((row) => row.id));
  const removeRow = db.prepare("DELETE FROM workspaces WHERE id = ?");
  for (const id of stored.keys()) if (!nextIds.has(id)) removeRow.run(id);
  const changed = [];
  next.forEach((row, position) => {
    const old = stored.get(row.id);
    if (!old || old.position !== position || old.data !== row.data)
      changed.push({ ...row, position });
  });
  const upsert = db.prepare(
    "INSERT INTO workspaces(id, position, endpoint, data) " +
      "VALUES(?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET " +
      "position = excluded.position, endpoint = excluded.endpoint, " +
      "data = excluded.data",
  );
  for (const row of changed)
    upsert.run(row.id, row.position, row.endpoint, row.data);
}

/** Stores an already parsed snapshot object (used by the one-time import,
 * which runs in its own transaction). */
function applySnapshot(db, snapshot) {
  if (!isObject(snapshot)) throw new Error("Invalid workspace snapshot");
  applyDelta(db, snapshot);
}

/** Stores the snapshot text in one transaction, writing only what changed. */
function writeSnapshotSync(dir, text) {
  if (typeof text !== "string" || Buffer.byteLength(text) > MAX_BYTES)
    throw new Error("Invalid workspace snapshot");
  const snapshot = JSON.parse(text);
  if (!isObject(snapshot)) throw new Error("Invalid workspace snapshot");
  const db = appDb(dir);
  transaction(db, () => applyDelta(db, snapshot));
}

/** `handle` is main.cjs's sender-checked ipcMain.handle wrapper; the
 * synchronous channels go through `on` and get the same sender check. */
function registerWorkspaceSnapshot({
  ipcMain,
  handle,
  getMainWindow,
  userDataDir,
  onWrite = () => {},
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
    onWrite();
    return null;
  });
  handle("workspace-state-write", (text) => {
    const mine = ++version;
    queue = queue
      .catch(() => {})
      .then(() => {
        if (mine !== version) return;
        writeSnapshotSync(userDataDir(), text);
        onWrite();
      });
    return queue;
  });
}

module.exports = {
  readSnapshot,
  savedWorkspaces,
  applySnapshot,
  writeSnapshotSync,
  registerWorkspaceSnapshot,
};
