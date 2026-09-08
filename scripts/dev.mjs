import { spawn } from "node:child_process";
import { watch } from "node:fs";
import { createServer } from "vite";

// Renderer (src/**) hot-reloads through Vite HMR.
// Main/preload (electron/**) can't hot-swap inside a running Electron,
// so the closest thing is an automatic restart; Vite stays up, so the
// window comes back on the same URL with the renderer already built.
const server = await createServer();
await server.listen();
const url = "http://127.0.0.1:5173";
let electron,
  restarting = false,
  timer;

function start() {
  electron = spawn("node_modules/.bin/electron", ["."], {
    stdio: "inherit",
    env: { ...process.env, BRIDGE_DEV_URL: url },
  });
  electron.on("exit", async (code) => {
    if (restarting) {
      restarting = false;
      return start();
    }
    await server.close();
    process.exit(code || 0);
  });
}
start();

// ponytail: fs.watch recursive, debounce 250 ms; add chokidar only if macOS coalescing misses edits
watch("electron", { recursive: true }, (_, file) => {
  if (!/\.(cjs|mjs|js|py)$/.test(file || "")) return;
  clearTimeout(timer);
  timer = setTimeout(() => {
    console.log(`\n[dev] electron/${file} changed → restarting Electron\n`);
    restarting = true;
    electron.kill();
  }, 250);
});

process.on("SIGINT", () => electron.kill());
process.on("SIGTERM", () => electron.kill());
