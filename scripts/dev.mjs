import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { createServer } from "vite";

const { exitAction } = createRequire(import.meta.url)(
  "../electron/dev-restart.cjs",
);

// The daemon (which hosts the orchestrator module) is a Rust binary the app
// spawns; build it first so the app talks to current code. A machine without
// cargo still gets a dev window.
const cargo = spawnSync("cargo", ["build", "--release", "-p", "sushiai"], {
  stdio: "inherit",
});
if (cargo.error || cargo.status !== 0)
  console.warn("[dev] sushiai daemon build skipped or failed.");

// The sessions live in the `sushiai` daemon the app starts from target/debug
// (or target/release, whichever is newer): build it so the app talks to
// current code. Without cargo the app uses whatever binary is already there.
const daemon = spawnSync("cargo", ["build", "-p", "sushiai"], {
  stdio: "inherit",
});
if (daemon.error || daemon.status !== 0)
  console.warn(
    "[dev] sushiai daemon build skipped or failed; sessions need a built binary.",
  );

// Renderer (src/**) hot-reloads through Vite HMR.
// Main/preload (electron/**) can't hot-swap inside a running Electron: main
// watches itself and offers a restart on the desktop mascot. Clicking it makes
// Electron exit with the restart code, and Vite stays up for the next window.
const server = await createServer();
await server.listen();
const url = "http://127.0.0.1:5173";
let electron;

// A dev run keeps its own daemon home, so it never fights an installed app
// (which may be an older build) over ~/.sushiai/daemon.sock. HOME and
// CODEX_HOME stay the real ones; SUSHIAI_DEV_SHARED=1 shares the installed
// app's daemon and sessions (both apps must then be the same build). Anything
// the caller already set wins.
function devEnv() {
  const vars = { ...process.env, BRIDGE_DEV_URL: url };
  if (process.env.SUSHIAI_DEV_SHARED === "1") return vars;
  const real = homedir();
  vars.HOME = vars.HOME || real;
  vars.CODEX_HOME = vars.CODEX_HOME || join(real, ".codex");
  // A shell started by the app exports the default home: that is not a choice.
  if (!vars.SUSHIAI_HOME || vars.SUSHIAI_HOME === join(real, ".sushiai")) {
    vars.SUSHIAI_HOME = join(real, ".sushiai-dev");
    mkdirSync(vars.SUSHIAI_HOME, { recursive: true, mode: 0o700 });
    // First run only: the catalog and settings; never sessions or state.
    for (const name of ["catalog.json", "settings.json"]) {
      const from = join(real, ".sushiai", name);
      const to = join(vars.SUSHIAI_HOME, name);
      if (existsSync(from) && !existsSync(to)) copyFileSync(from, to);
    }
    // The app's own data (workspaces, panes) too: a shared one made the dev
    // app reopen the installed app's panes and resume their agent sessions a
    // second time in its own daemon.
    if (!vars.BRIDGE_DATA_DIR) {
      vars.BRIDGE_DATA_DIR = join(vars.SUSHIAI_HOME, "app-data");
      seedAppData(vars.BRIDGE_DATA_DIR, join(real, INSTALLED_DATA));
    }
  }
  return vars;
}

const INSTALLED_DATA = "Library/Application Support/sushiai-workspace";

/** First run only: the installed app's projects and ssh hosts, as the legacy
 * files the app imports on open. Never workspaces, so no pane or session. */
function seedAppData(dir, installed) {
  if (existsSync(dir)) return;
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const file = join(installed, "sushiai.db");
  if (!existsSync(file)) return;
  let db;
  try {
    db = new DatabaseSync(file, { readOnly: true });
    const projects = {};
    for (const row of db.prepare("SELECT id, data FROM projects").all())
      projects[row.id] = { ...JSON.parse(row.data), folders: [] };
    for (const row of db
      .prepare("SELECT host, path, project_id FROM folders")
      .all())
      projects[row.project_id]?.folders.push({
        endpoint: row.host,
        cwd: row.path,
      });
    const hosts = db
      .prepare("SELECT data FROM connections ORDER BY position")
      .all()
      .map((row) => JSON.parse(row.data));
    writeFileSync(join(dir, "projects.json"), JSON.stringify(projects));
    writeFileSync(join(dir, "connections.json"), JSON.stringify(hosts));
  } catch (error) {
    console.warn(`[dev] could not copy projects: ${error.message}`);
  } finally {
    db?.close();
  }
}

function start() {
  electron = spawn("node_modules/.bin/electron", ["."], {
    stdio: "inherit",
    env: devEnv(),
  });
  electron.on("exit", async (code) => {
    if (exitAction(code) === "respawn") {
      console.log("\n[dev] restarting Electron\n");
      return start();
    }
    await server.close();
    process.exit(code || 0);
  });
}
start();

process.on("SIGINT", () => electron.kill());
process.on("SIGTERM", () => electron.kill());
