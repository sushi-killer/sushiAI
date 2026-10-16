import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
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
  if (!vars.SUSHIAI_HOME) {
    vars.SUSHIAI_HOME = join(real, ".sushiai-dev");
    mkdirSync(vars.SUSHIAI_HOME, { recursive: true, mode: 0o700 });
    // First run only: the catalog and settings; never sessions or state.
    for (const name of ["catalog.json", "settings.json"]) {
      const from = join(real, ".sushiai", name);
      const to = join(vars.SUSHIAI_HOME, name);
      if (existsSync(from) && !existsSync(to)) copyFileSync(from, to);
    }
  }
  return vars;
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
