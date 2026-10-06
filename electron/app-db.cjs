const fs = require("node:fs");
const path = require("node:path");
const { DatabaseSync } = require("node:sqlite");

/** Runs `work` in one transaction: it commits when `work` returns and rolls
 * back when it throws. */
function transaction(db, work) {
  db.exec("BEGIN IMMEDIATE");
  try {
    const result = work();
    db.exec("COMMIT");
    return result;
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

const MIGRATIONS = [
  // v1: projects and their folders.
  `CREATE TABLE IF NOT EXISTS projects(
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS folders(
    host TEXT NOT NULL,
    path TEXT NOT NULL,
    project_id TEXT,
    remote_key TEXT,
    common_dir TEXT,
    checkout TEXT,
    linked_worktree INTEGER,
    subdir TEXT,
    branch TEXT,
    seen_at INTEGER,
    PRIMARY KEY(host, path)
  );`,
  // v2: the stores that used to be JSON files.
  `CREATE TABLE workspaces(
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    endpoint TEXT NOT NULL DEFAULT '',
    herdr_id TEXT,
    data TEXT NOT NULL
  );
  CREATE UNIQUE INDEX workspaces_herdr ON workspaces(endpoint, herdr_id)
    WHERE herdr_id IS NOT NULL AND herdr_id != '';
  CREATE TABLE app_state(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
  );
  CREATE TABLE launches(
    endpoint TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(endpoint, operation_id)
  );
  CREATE TABLE connections(
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    data TEXT NOT NULL
  );
  CREATE TABLE orchestrator_hosts(host TEXT PRIMARY KEY);`,
  // v3: small named key -> JSON value stores (providers, secrets, accounts,
  // window and update settings) that used to be one JSON file each.
  `CREATE TABLE store(
    name TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(name, key)
  );`,
  // v4: a workspace is bound to its daemon sessions through its panels and
  // its id is the daemon group, so the Herdr binding column and its unique
  // index go. The rows keep their order and data.
  `CREATE TABLE workspaces_v4(
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    endpoint TEXT NOT NULL DEFAULT '',
    data TEXT NOT NULL
  );
  INSERT INTO workspaces_v4(id, position, endpoint, data)
    SELECT id, position, endpoint, data FROM workspaces;
  DROP TABLE workspaces;
  ALTER TABLE workspaces_v4 RENAME TO workspaces;`,
];

function migrate(db) {
  transaction(db, () => {
    const current = db.prepare("PRAGMA user_version").get().user_version;
    for (let version = current; version < MIGRATIONS.length; version += 1) {
      db.exec(MIGRATIONS[version]);
      db.exec(`PRAGMA user_version = ${version + 1}`);
    }
  });
}

/** Reads a leftover JSON file once. Returns undefined (and warns) when it is
 * missing or unreadable, so a bad file never blocks opening the database. */
function readLegacy(file) {
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT")
      console.warn(`Not importing ${path.basename(file)}: unreadable`);
    return undefined;
  }
  // A parse error can quote the text, and this text may hold secrets.
  try {
    return JSON.parse(text);
  } catch {
    console.warn(`Not importing ${path.basename(file)}: not valid JSON`);
    return undefined;
  }
}

const isEmpty = (db, table) =>
  !db.prepare(`SELECT 1 FROM ${table} LIMIT 1`).get();
const isObject = (value) =>
  Boolean(value) && typeof value === "object" && !Array.isArray(value);

/** What a store row holds when its legacy file exists but cannot be taken in:
 * not JSON, so it reads back as DAMAGED and the owner fails closed. */
const DAMAGED_TEXT = "damaged";
/** The value `readStore(..., { damaged: true })` gives a row that is present
 * but unreadable, as opposed to a key that is absent. */
const DAMAGED = Symbol("damaged store row");

/** Takes in one legacy file: `apply(parsed)` runs in a transaction and
 * returns false to refuse the content; once committed the file is renamed to
 * `<name>.imported`, or deleted when `remove` is set (files holding secrets;
 * a deletion that failed is retried on the next open). `damaged(db)` runs when
 * the file exists but cannot be read or is refused, and leaves the file. */
function importLegacy(
  db,
  userDataDir,
  name,
  tables,
  apply,
  { remove = false, marker } = {},
) {
  const file = path.join(userDataDir, name);
  const imported = `${file}.imported`;
  // Only a committed import renames the file; a secret file's renamed copy is
  // then removed, and a removal that failed is retried on the next open. The
  // file itself is never removed, so one that was not imported stays.
  const drop = () => {
    try {
      fs.rmSync(imported, { force: true });
    } catch {
      console.warn(`Could not remove ${name}.imported`);
    }
  };
  if (remove && fs.existsSync(imported)) drop();
  if (
    !tables.every((table) =>
      typeof table === "function" ? table(db) : isEmpty(db, table),
    )
  )
    return;
  const parsed = readLegacy(file);
  if (parsed === undefined) {
    // A damaged file marks its key; once the owner removes the file, the
    // marker goes too and the store starts over from defaults.
    if (marker && fs.existsSync(file)) markDamaged(db, ...marker);
    else if (marker) clearDamaged(db, ...marker);
    return;
  }
  try {
    const accepted = transaction(db, () => apply(parsed) !== false);
    if (!accepted) {
      console.warn(`Not importing ${name}: unexpected content`);
      if (marker) markDamaged(db, ...marker);
      return;
    }
    fs.renameSync(file, imported);
    if (remove) drop();
  } catch (error) {
    console.warn(`Not importing ${name}: ${error?.code || "failed"}`);
  }
}

const storeIsEmpty = (name) => (db) =>
  !db.prepare("SELECT 1 FROM store WHERE name = ? LIMIT 1").get(name);

/** The rows of store `name` as a plain key -> value object. A row that does
 * not parse is skipped with a warning, or given as DAMAGED with `damaged`. */
function readStoreFrom(db, name, { damaged = false } = {}) {
  const result = Object.create(null);
  for (const row of db
    .prepare("SELECT key, value FROM store WHERE name = ? ORDER BY rowid")
    .all(name)) {
    try {
      result[row.key] = JSON.parse(row.value);
    } catch {
      console.warn(`Skipping unreadable ${name} entry ${row.key}`);
      if (damaged) result[row.key] = DAMAGED;
    }
  }
  return result;
}

/** Makes store `name` equal `object`: changed keys are written, missing keys
 * deleted, unchanged ones left alone. Runs inside the caller's transaction. */
function putStore(db, name, object) {
  const stored = new Map(
    db
      .prepare("SELECT key, value FROM store WHERE name = ?")
      .all(name)
      .map((row) => [row.key, row.value]),
  );
  for (const [key, text] of stored)
    if (!Object.hasOwn(object, key) || object[key] === undefined) {
      // A row that does not parse never reached the caller, so its absence
      // from `object` is not a deletion: it is left for inspection.
      try {
        JSON.parse(text);
      } catch {
        continue;
      }
      db.prepare("DELETE FROM store WHERE name = ? AND key = ?").run(name, key);
    }
  for (const [key, value] of Object.entries(object)) {
    if (value === undefined) continue;
    const text = JSON.stringify(value);
    if (stored.get(key) !== text)
      db.prepare(
        "INSERT INTO store(name, key, value) VALUES(?, ?, ?) ON CONFLICT(name, key) DO UPDATE SET value = excluded.value",
      ).run(name, key, text);
  }
}

const readStore = (userDataDir, name, options) =>
  readStoreFrom(appDb(userDataDir), name, options);

const putKey = (db, name, key, value) =>
  db
    .prepare(
      "INSERT INTO store(name, key, value) VALUES(?, ?, ?) ON CONFLICT(name, key) DO UPDATE SET value = excluded.value",
    )
    .run(name, key, JSON.stringify(value));

/** Records that the legacy file behind one key is there but unreadable. */
const markDamaged = (db, name, key) =>
  db
    .prepare(
      "INSERT INTO store(name, key, value) VALUES(?, ?, ?) ON CONFLICT(name, key) DO UPDATE SET value = excluded.value",
    )
    .run(name, key, DAMAGED_TEXT);

const clearDamaged = (db, name, key) =>
  db
    .prepare("DELETE FROM store WHERE name = ? AND key = ? AND value = ?")
    .run(name, key, DAMAGED_TEXT);

/** True for a key with no row, or one only holding the damaged marker. */
const keyIsOpen = (name, key) => (db) => {
  const row = db
    .prepare("SELECT value FROM store WHERE name = ? AND key = ?")
    .get(name, key);
  return !row || row.value === DAMAGED_TEXT;
};

const writeStore = (userDataDir, name, object) => {
  const db = appDb(userDataDir);
  transaction(db, () => putStore(db, name, object));
};

/** One launch record as the launcher stores it; false for anything else. */
function validLaunchRecord(record) {
  return (
    isObject(record) &&
    typeof record.endpoint === "string" &&
    typeof record.operationId === "string" &&
    isObject(record.created) &&
    ["workspaceId", "paneId", "cwd"].every(
      (field) => typeof record.created[field] === "string",
    ) &&
    typeof record.signatureHash === "string" &&
    typeof record.preparationHash === "string"
  );
}

function importAll(db, userDataDir) {
  importLegacy(db, userDataDir, "projects.json", ["projects"], (parsed) => {
    if (!isObject(parsed)) return false;
    const insertProject = db.prepare(
      "INSERT INTO projects(id, data) VALUES(?, ?)",
    );
    const insertFolder = db.prepare(
      "INSERT OR IGNORE INTO folders(host, path, project_id) VALUES(?, ?, ?)",
    );
    for (const [id, project] of Object.entries(parsed)) {
      if (!project || typeof project !== "object") continue;
      const { folders = [], ...data } = project;
      insertProject.run(id, JSON.stringify({ ...data, id }));
      for (const folder of folders)
        insertFolder.run(String(folder.endpoint), String(folder.cwd), id);
    }
  });
  const { applySnapshot } = require("./workspace-snapshot.cjs");
  importLegacy(
    db,
    userDataDir,
    "workspace-state.json",
    ["workspaces", "app_state"],
    (parsed) => {
      if (!isObject(parsed)) return false;
      applySnapshot(db, parsed);
      return true;
    },
  );
  importLegacy(
    db,
    userDataDir,
    "herdr-launches.json",
    ["launches"],
    (parsed) => {
      if (!isObject(parsed) || !Array.isArray(parsed.operations)) return false;
      const insert = db.prepare(
        "INSERT OR REPLACE INTO launches(endpoint, operation_id, data) VALUES(?, ?, ?)",
      );
      for (const record of parsed.operations) {
        if (validLaunchRecord(record))
          insert.run(
            record.endpoint,
            record.operationId,
            JSON.stringify(record),
          );
        else console.warn("Skipping an invalid launch record in the import");
      }
    },
  );
  importLegacy(
    db,
    userDataDir,
    "connections.json",
    ["connections"],
    (parsed) => {
      if (!Array.isArray(parsed)) return false;
      const insert = db.prepare(
        "INSERT OR REPLACE INTO connections(id, position, data) VALUES(?, ?, ?)",
      );
      const { validate } = require("./connections.cjs");
      let position = 0;
      for (const profile of parsed) {
        try {
          const valid = validate(profile);
          insert.run(valid.id, position, JSON.stringify(valid));
          position += 1;
        } catch (error) {
          console.warn(`Skipping an invalid connection: ${error.message}`);
        }
      }
    },
  );
  importLegacy(
    db,
    userDataDir,
    "orchestrator-hosts.json",
    ["orchestrator_hosts"],
    (parsed) => {
      if (!Array.isArray(parsed)) return false;
      const insert = db.prepare(
        "INSERT OR IGNORE INTO orchestrator_hosts(host) VALUES(?)",
      );
      for (const host of parsed) if (typeof host === "string") insert.run(host);
    },
  );
  // Key -> value files: each top-level key becomes a row. The first four hold
  // secrets or their index, so the old file is deleted, not kept.
  for (const [name, file, remove] of [
    ["providers", "providers.json", false],
    ["secrets", "secrets.json", true],
    ["model-profiles", "model-profiles.json", false],
    ["claude-accounts", "claude-accounts.json", true],
    ["codex-accounts", "codex-accounts.json", true],
    ["project-secrets", "project-secrets.json", true],
    ["hermes-scheduler", "agents/hermes-scheduler.json", false],
  ])
    importLegacy(
      db,
      userDataDir,
      file,
      [storeIsEmpty(name)],
      (parsed) => {
        if (!isObject(parsed)) return false;
        putStore(db, name, parsed);
      },
      { remove },
    );
  // Single-document files keep their whole content under the key `value`.
  for (const [name, file] of [
    ["window", "window-state.json"],
    ["updates", "updates.json"],
    ["preferences", "app-preferences.json"],
  ])
    importLegacy(db, userDataDir, file, [storeIsEmpty(name)], (parsed) => {
      if (!isObject(parsed)) return false;
      putStore(db, name, { value: parsed });
    });
  // Extension state fails closed: a legacy file that cannot be taken in leaves
  // a damaged marker row, so the manager does not start over from defaults,
  // and the import is retried on every open until the file is repaired.
  for (const [name, file] of [
    ["extensions", "extensions/extensions.json"],
    ["extension-lock", "extensions/extension-lock.json"],
  ])
    importLegacy(
      db,
      userDataDir,
      file,
      [keyIsOpen(name, "value")],
      (parsed) => {
        if (!isObject(parsed)) return false;
        putStore(db, name, { value: parsed });
      },
      { marker: [name, "value"] },
    );
  importLegacy(
    db,
    userDataDir,
    "agents/hermes-activity.json",
    [storeIsEmpty("hermes-activity")],
    (parsed) => {
      if (!Array.isArray(parsed)) return false;
      const rows = parsed.filter(
        (row) => isObject(row) && row.id !== undefined,
      );
      putStore(db, "hermes-activity", activityRows(rows));
    },
  );
  // Surface state: one file per extension, one row per extension.
  const stateDir = path.join(userDataDir, "extensions", "state");
  let names = [];
  try {
    names = fs.readdirSync(stateDir);
  } catch {}
  for (const { key } of db
    .prepare("SELECT key FROM store WHERE name = 'surface-state' AND value = ?")
    .all(DAMAGED_TEXT))
    if (!names.some((file) => file.replace(/\.json$/, "") === key))
      clearDamaged(db, "surface-state", key);
  for (const file of names) {
    const id = file.replace(/\.json$/, "");
    if (id === file || !/^[a-z0-9][a-z0-9._-]*$/.test(id)) continue;
    importLegacy(
      db,
      userDataDir,
      path.join("extensions", "state", file),
      [keyIsOpen("surface-state", id)],
      (parsed) => {
        if (!isObject(parsed)) return false;
        putKey(db, "surface-state", id, parsed);
      },
      { marker: ["surface-state", id] },
    );
  }
}

/** The journal as store rows: one per entry plus the order they were kept in. */
function activityRows(entries) {
  const rows = { order: entries.map((entry) => String(entry.id)) };
  for (const entry of entries) rows[`e:${entry.id}`] = entry;
  return rows;
}

const open = new Map();

/** The one `<userDataDir>/sushiai.db` connection of this process (WAL),
 * created on first use with its tables migrated and leftover JSON files taken
 * in. No import ever throws: a bad file is kept and warned about. */
function appDb(userDataDir) {
  const key = path.resolve(userDataDir);
  const cached = open.get(key);
  if (cached) return cached;
  fs.mkdirSync(userDataDir, { recursive: true, mode: 0o700 });
  const file = path.join(userDataDir, "sushiai.db");
  // A second process holding a write is waited for, not failed at once.
  const db = new DatabaseSync(file, { timeout: 5000 });
  try {
    db.exec("PRAGMA journal_mode=WAL");
    // Owner-only like the JSON stores it replaces; SQLite gives the WAL and
    // shared-memory files the database file's mode.
    for (const suffix of ["", "-wal", "-shm"])
      if (fs.existsSync(file + suffix)) fs.chmodSync(file + suffix, 0o600);
    migrate(db);
    importAll(db, userDataDir);
  } catch (error) {
    db.close();
    throw error;
  }
  open.set(key, db);
  return db;
}

/** Closes and forgets the connection of `userDataDir` (tests, reopen). */
function closeAppDb(userDataDir) {
  const key = path.resolve(userDataDir);
  open.get(key)?.close();
  open.delete(key);
}

module.exports = {
  DAMAGED,
  appDb,
  closeAppDb,
  activityRows,
  putStore,
  readStore,
  transaction,
  validLaunchRecord,
  writeStore,
};
