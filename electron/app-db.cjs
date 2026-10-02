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
      console.warn(`Not importing ${path.basename(file)}: ${error.message}`);
    return undefined;
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    console.warn(`Not importing ${path.basename(file)}: ${error.message}`);
    return undefined;
  }
}

const isEmpty = (db, table) =>
  !db.prepare(`SELECT 1 FROM ${table} LIMIT 1`).get();
const isObject = (value) =>
  Boolean(value) && typeof value === "object" && !Array.isArray(value);

/** Takes in one legacy file: `apply(db, parsed)` runs in a transaction and
 * returns false to refuse the content; on success the file is renamed. */
function importLegacy(db, userDataDir, name, tables, apply) {
  if (!tables.every((table) => isEmpty(db, table))) return;
  const file = path.join(userDataDir, name);
  const parsed = readLegacy(file);
  if (parsed === undefined) return;
  try {
    const accepted = transaction(db, () => apply(parsed) !== false);
    if (!accepted) {
      console.warn(`Not importing ${name}: unexpected content`);
      return;
    }
    fs.renameSync(file, `${file}.imported`);
  } catch (error) {
    console.warn(`Not importing ${name}: ${error.message}`);
  }
}

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

module.exports = { appDb, closeAppDb, transaction, validLaunchRecord };
