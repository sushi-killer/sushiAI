import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { createServer } from "vite";

const { exitAction } = createRequire(import.meta.url)(
  "../electron/dev-restart.cjs",
);

// The orchestrator daemon is a Rust binary the app spawns; build it first so
// the panel talks to current code. A machine without cargo still gets a dev
// window, with the panel showing its "not built" state.
const cargo = spawnSync(
  "cargo",
  ["build", "--release", "-p", "sushiai-orch", "--bin", "orchd"],
  {
    stdio: "inherit",
  },
);
if (cargo.error || cargo.status !== 0)
  console.warn(
    "[dev] orchd build skipped or failed; the Orchestrator panel will say so.",
  );

// Renderer (src/**) hot-reloads through Vite HMR.
// Main/preload (electron/**) can't hot-swap inside a running Electron: main
// watches itself and offers a restart on the desktop mascot. Clicking it makes
// Electron exit with the restart code, and Vite stays up for the next window.
const server = await createServer();
await server.listen();
const url = "http://127.0.0.1:5173";
let electron;

function start() {
  electron = spawn("node_modules/.bin/electron", ["."], {
    stdio: "inherit",
    env: { ...process.env, BRIDGE_DEV_URL: url },
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
