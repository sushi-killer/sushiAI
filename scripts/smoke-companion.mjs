// Desktop proof of the companion contract with the synthetic fixture: an
// extension in a folder outside the app (symlinked into the local extensions
// folder of a temp profile) shows "needs approval", runs only after the owner
// approves its resolved program, shows its status and QR in a Settings tab, and
// its process ends when the extension is disabled. Everything lives in a temp
// profile; the owner's ~/.sushiai is never read or written.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { daemonBinary, stopDaemon } from "./lib/daemon-binary.mjs";

const root = process.cwd();
const fixture = path.join(root, "tests/fixtures/extensions/companion-probe");
const manifest = JSON.parse(
  await fs.readFile(path.join(fixture, "manifest.json"), "utf8"),
);
const surface = manifest.contributions.surfaces[0];
const profile = await fs.mkdtemp("/tmp/sushiai-companion-");
const home = path.join(profile, "sushiai");
const foreign = path.join(profile, "foreign", "companion-probe");
const startsFile = path.join(home, "probe", "starts");

async function until(read, ready, timeout = 15000) {
  const deadline = Date.now() + timeout;
  let value = await read();
  while (!ready(value) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    value = await read();
  }
  return value;
}
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const readStarts = async () =>
  (await fs.readFile(startsFile, "utf8").catch(() => ""))
    .split("\n")
    .filter(Boolean)
    .map(Number);

// The manifest in a foreign folder; the program is a bare name that resolves
// in $SUSHIAI_HOME/bin, never inside the extension folder.
await fs.mkdir(foreign, { recursive: true });
await fs.copyFile(
  path.join(fixture, "manifest.json"),
  path.join(foreign, "manifest.json"),
);
await fs.mkdir(path.join(profile, "local-extensions"), { recursive: true });
await fs.symlink(
  foreign,
  path.join(profile, "local-extensions", "companion-probe"),
);
await fs.mkdir(path.join(home, "bin"), { recursive: true });
const program = path.join(home, "bin", manifest.companion.command);
await fs.writeFile(
  program,
  `#!/bin/sh\nexec "${process.execPath}" "${path.join(fixture, "companion.cjs")}" "$@"\n`,
  { mode: 0o755 },
);

const desktop = await electron.launch({
  args: ["."],
  cwd: root,
  env: {
    ...process.env,
    BRIDGE_DATA_DIR: profile,
    HOME: profile,
    CODEX_HOME: path.join(profile, "codex"),
    SUSHIAI_HOME: home,
    SUSHIAI_DAEMON_BIN: process.env.SUSHIAI_DAEMON_BIN || daemonBinary(root),
    SUSHIAI_EXTENSIONS_DIR: "",
    BRIDGE_DEV_URL: "",
    SUSHIAI_TEST_WINDOW: "hidden",
  },
});
const errors = [];
// UI evidence: SUSHIAI_SHOT_DIR names a folder for the approval dialog and the
// settings tab screenshots.
const shot = (page, name) =>
  process.env.SUSHIAI_SHOT_DIR
    ? page.screenshot({ path: path.join(process.env.SUSHIAI_SHOT_DIR, name) })
    : undefined;
try {
  const page = await desktop.firstWindow();
  page.on("pageerror", (error) => errors.push(error.message));
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  const card = page
    .locator(".extension-card")
    .filter({ hasText: manifest.name });
  await card.waitFor();
  await page
    .getByRole("button", { name: `Enable ${manifest.name}`, exact: true })
    .click();
  // Enabled but not approved: nothing runs.
  await card
    .locator(".extension-companion-state")
    .filter({ hasText: "Needs your approval" })
    .waitFor();
  assert.deepEqual(await readStarts(), [], "no process before approval");

  await card
    .getByRole("button", { name: `Review the program of ${manifest.name}` })
    .click();
  const dialog = page.getByRole("dialog", { name: `Approve ${manifest.name}` });
  await dialog.waitFor();
  const shown = await dialog.locator(".extension-approval-facts").innerText();
  assert.ok(
    shown.includes(await fs.realpath(program)) || shown.includes(program),
    "the dialog shows the resolved program",
  );
  assert.ok(shown.includes(manifest.companion.args.join(" ")));
  assert.deepEqual(await readStarts(), [], "no process while reviewing");
  await shot(page, "s7-polish-approval.png");
  await dialog.getByRole("button", { name: "Approve", exact: true }).click();
  await dialog.waitFor({ state: "detached" });
  await card
    .locator(".extension-companion-state")
    .filter({ hasText: "Running" })
    .waitFor();
  await page
    .locator(".extensions-view")
    .evaluate((view) => view.parentElement?.scrollTo(0, 0));
  await shot(page, "s7-polish-extensions-running.png");
  const [pid] = await until(readStarts, (list) => list.length > 0);
  assert.ok(pid && alive(pid), "the companion runs after approval");

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.waitFor();
  await settings.getByRole("tab", { name: surface.title, exact: true }).click();
  await settings
    .locator(".companion-status")
    .filter({ hasText: "Connected" })
    .waitFor();
  await settings.locator("svg[role=img]").first().waitFor();
  await shot(page, "s7-polish-companion-settings.png");
  await page.getByRole("button", { name: "Close dialog" }).click();

  await page
    .getByRole("button", { name: `Disable ${manifest.name}`, exact: true })
    .click();
  const gone = await until(
    () => alive(pid),
    (value) => !value,
    5000,
  );
  assert.equal(gone, false, "disable stops the companion process");
  assert.deepEqual(errors, [], "no renderer errors");
  console.log(JSON.stringify({ companion: "ok", pid }));
} catch (error) {
  const page = await desktop.firstWindow();
  await page.screenshot({ path: path.join(root, "artifacts/failure.png") });
  console.error("Renderer errors:", errors);
  throw error;
} finally {
  await desktop.close();
  stopDaemon(home);
  await fs.rm(profile, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 200,
  });
}
