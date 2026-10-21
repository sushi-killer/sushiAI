// Local agents attach to the owner's live `npm run dev` window to photograph
// and measure it with real data. Only scripts/dev.mjs passes these flags: the
// packaged app never opens a debugging port (scripts/check-conventions.mjs).
//
// SECURITY: while dev runs, ANY local process can drive the dev window over
// this port (loopback only, port chosen by Chromium). The 0600 mode on
// devtools.json only limits who can DISCOVER the port; it protects nothing.
// The owner asked for this on by default in dev; SUSHIAI_DEV_ATTACH=0 turns
// it off.
//
// The endpoint is published at one fixed path, ~/.sushiai-dev/devtools.json,
// whatever SUSHIAI_HOME the dev run uses, so the writer and the readers never
// disagree (SUSHIAI_DEV_SHARED included).
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export const DEVTOOLS_FLAGS = [
  "--remote-debugging-port=0",
  "--remote-debugging-address=127.0.0.1",
];

/** The renderer origin `npm run dev` serves (vite.config.ts port). */
export const DEV_URL = "http://127.0.0.1:5173";

const LISTENING = /DevTools listening on (ws:\/\/127\.0\.0\.1:(\d+)\/\S+)/;

export function attachEnabled(env = process.env) {
  return env.SUSHIAI_DEV_ATTACH !== "0";
}

/** SUSHIAI_DEVTOOLS_FILE is a seam for tests only. */
export function devtoolsFile(env = process.env) {
  return (
    env.SUSHIAI_DEVTOOLS_FILE ||
    join(homedir(), ".sushiai-dev", "devtools.json")
  );
}

export function parseListening(line) {
  const match = LISTENING.exec(line);
  return match ? { wsEndpoint: match[1], port: Number(match[2]) } : null;
}

/** Tees the child's stderr to ours, publishes the endpoint when a whole line
 * announces it, and removes the file when the child exits. */
export function publishDevtools(child, file = devtoolsFile()) {
  let published = false;
  let pending = "";
  child.stderr.on("data", (chunk) => {
    process.stderr.write(chunk);
    if (published) return;
    pending += chunk.toString();
    const lines = pending.split("\n");
    pending = lines.pop();
    for (const line of lines) {
      const found = parseListening(line);
      if (!found) continue;
      published = true;
      mkdirSync(dirname(file), { recursive: true, mode: 0o700 });
      writeFileSync(file, JSON.stringify({ ...found, pid: child.pid }), {
        mode: 0o600,
      });
      chmodSync(file, 0o600);
      return;
    }
  });
  child.on("exit", () => rmSync(file, { force: true }));
}

/** Any failure of the pid probe means the file is stale; EPERM too, since a
 * live process of ours would answer. */
export function readDevtools(file = devtoolsFile(), probe = process.kill) {
  if (!existsSync(file))
    throw new Error(
      `${file} is missing: start the app with \`npm run dev\` first`,
    );
  const info = JSON.parse(readFileSync(file, "utf8"));
  try {
    probe(info.pid, 0);
  } catch {
    throw new Error(`${file} is stale: process ${info.pid} is not running`);
  }
  return info;
}
