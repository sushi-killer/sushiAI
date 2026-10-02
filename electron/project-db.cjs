const fs = require("node:fs");
const path = require("node:path");
const { DatabaseSync } = require("node:sqlite");

const VERSION = 1;

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

/** A first open with an empty store takes in the old projects.json (projects
 * and their folders) in one transaction, then renames it. A failure rolls the
 * import back, leaves the file where it was and throws, so the next start
 * tries again. */
function importProjects(db, file) {
  if (db.prepare("SELECT 1 FROM projects LIMIT 1").get()) return;
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw error;
  }
  const parsed = JSON.parse(text);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
    throw new Error("projects.json is not a project map.");
  const insertProject = db.prepare(
    "INSERT INTO projects(id, data) VALUES(?, ?)",
  );
  const insertFolder = db.prepare(
    "INSERT OR IGNORE INTO folders(host, path, project_id) VALUES(?, ?, ?)",
  );
  transaction(db, () => {
    for (const [id, project] of Object.entries(parsed)) {
      if (!project || typeof project !== "object") continue;
      const { folders = [], ...data } = project;
      insertProject.run(id, JSON.stringify({ ...data, id }));
      for (const folder of folders)
        insertFolder.run(String(folder.endpoint), String(folder.cwd), id);
    }
  });
  fs.renameSync(file, `${file}.imported`);
}

/** Opens `<userDataDir>/sushiai.db` (WAL), creating the tables and taking in
 * a leftover projects.json. */
function openProjectDb(userDataDir) {
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
    transaction(db, () => {
      if (db.prepare("PRAGMA user_version").get().user_version >= VERSION)
        return;
      db.exec(`
        CREATE TABLE IF NOT EXISTS projects(
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
        );
        PRAGMA user_version = ${VERSION};
      `);
    });
    importProjects(db, path.join(userDataDir, "projects.json"));
  } catch (error) {
    db.close();
    throw error;
  }
  return db;
}

module.exports = { openProjectDb, transaction };
