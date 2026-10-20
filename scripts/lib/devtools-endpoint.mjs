// Local agents attach to the owner's live `npm run dev` window to photograph
// and measure it with real data. Only scripts/dev.mjs passes these flags: the
// packaged app never opens a debugging port. The port is bound to loopback and
// chosen by Chromium (0); the endpoint is published in
// $SUSHIAI_HOME/devtools.json (mode 0600) while that Electron runs.
import {
  chmodSync,
  existsSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export const DEVTOOLS_FLAGS = [
  "--remote-debugging-port=0",
  "--remote-debugging-address=127.0.0.1",
];

const LISTENING = /DevTools listening on (ws:\/\/127\.0\.0\.1:(\d+)\/\S+)/;

export function devtoolsFile(home) {
  return join(home, "devtools.json");
}

/** Where agents look: SUSHIAI_HOME, else the dev home `npm run dev` uses. */
export function defaultHome(env = process.env) {
  return env.SUSHIAI_HOME || join(homedir(), ".sushiai-dev");
}

export function parseListening(text) {
  const match = LISTENING.exec(text);
  return match ? { wsEndpoint: match[1], port: Number(match[2]) } : null;
}

/** Tees the child's stderr to ours, publishes the endpoint once Chromium
 * prints it, and removes the file when the child exits. */
export function publishDevtools(child, home) {
  const file = devtoolsFile(home);
  let published = false;
  child.stderr.on("data", (chunk) => {
    process.stderr.write(chunk);
    if (published) return;
    const found = parseListening(chunk.toString());
    if (!found) return;
    published = true;
    writeFileSync(file, JSON.stringify({ ...found, pid: child.pid }), {
      mode: 0o600,
    });
    chmodSync(file, 0o600);
  });
  child.on("exit", () => rmSync(file, { force: true }));
}

export function readDevtools(home) {
  const file = devtoolsFile(home);
  if (!existsSync(file))
    throw new Error(
      `${file} is missing: start the app with \`npm run dev\` first`,
    );
  const info = JSON.parse(readFileSync(file, "utf8"));
  try {
    process.kill(info.pid, 0);
  } catch (error) {
    if (error.code === "ESRCH")
      throw new Error(`${file} is stale: process ${info.pid} is gone`);
  }
  return info;
}
