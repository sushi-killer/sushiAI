// The Chromium the browser scripts launch: TERMINAL_TEST_BROWSER, then
// Playwright's own build, then the newest installed headless shell or Chromium
// when Playwright expects a build that is not installed. Undefined lets
// Playwright report what is missing.
import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";

const BUILDS = [
  [
    "chromium_headless_shell-",
    "chrome-headless-shell-linux64/chrome-headless-shell",
  ],
  ["chromium-", "chrome-linux64/chrome"],
];

export function resolveBrowser(chromium) {
  if (process.env.TERMINAL_TEST_BROWSER)
    return process.env.TERMINAL_TEST_BROWSER;
  const expected = chromium.executablePath();
  if (expected && existsSync(expected)) return expected;
  const base =
    process.env.PLAYWRIGHT_BROWSERS_PATH ||
    path.join(homedir(), ".cache", "ms-playwright");
  let entries = [];
  try {
    entries = readdirSync(base);
  } catch {
    return undefined;
  }
  const found = [];
  for (const [prefix, binary] of BUILDS) {
    for (const entry of entries) {
      const revision = Number(entry.slice(prefix.length));
      if (!entry.startsWith(prefix) || !Number.isInteger(revision)) continue;
      const file = path.join(base, entry, binary);
      if (existsSync(file)) found.push({ revision, file });
    }
  }
  // Newest revision first; the headless shell wins a tie (it is listed first).
  found.sort((a, b) => b.revision - a.revision);
  return found[0]?.file;
}

// Electron's launch arguments for a test or evidence run. A Linux host with
// no display (an orchd sandbox blocks the X socket) draws headless, so
// screenshots still render.
export const ELECTRON_ARGS = [
  ".",
  ...(process.platform === "linux" &&
  !process.env.DISPLAY &&
  !process.env.WAYLAND_DISPLAY
    ? ["--ozone-platform=headless"]
    : []),
];
