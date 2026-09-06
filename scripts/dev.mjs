import { spawn } from "node:child_process";
import { createServer } from "vite";
const server = await createServer();
await server.listen();
const electron = spawn("node_modules/.bin/electron", ["."], {
  stdio: "inherit",
  env: { ...process.env, BRIDGE_DEV_URL: "http://127.0.0.1:5173" },
});
electron.on("exit", async (code) => {
  await server.close();
  process.exit(code || 0);
});
process.on("SIGINT", () => electron.kill());
process.on("SIGTERM", () => electron.kill());
