import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
const profile = await fs.mkdtemp("/tmp/sushiai-updates-test-");
const artifacts = path.resolve("artifacts");
await fs.mkdir(artifacts, { recursive: true });
const app = await electron.launch({
  ...(process.env.SUSHIAI_EXECUTABLE
    ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
    : {}),
  args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
  env: { ...process.env, SUSHIAI_TEST_HEADLESS: "1", BRIDGE_DATA_DIR: profile },
});
try {
  const page = await app.firstWindow();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.waitForSelector(".panel");
  const installedState = await page.evaluate(() =>
    window.bridge.updatesState(),
  );
  assert.equal(installedState.repository, "sushi-killer/sushiAI");
  assert.equal(typeof installedState.settings.autoDownload, "boolean");
  await app.evaluate(async ({ ipcMain, BrowserWindow }, root) => {
    const { createRequire } = process.getBuiltinModule("node:module");
    const require = createRequire(root + "/package.json");
    const { Updates } = require(root + "/electron/updates.cjs");
    const { createHash } = require("node:crypto");
    const bytes = Buffer.from("synthetic update fixture");
    const name = "sushiAI-0.0.2-alpha.1-arm64.dmg";
    const releases = [
      {
        tag_name: "v0.0.2-alpha.1",
        prerelease: true,
        body: "Live agents and Git statistics.\nAutomatic update downloads.",
        assets: [
          {
            name,
            state: "uploaded",
            size: bytes.length,
            digest:
              "sha256:" + createHash("sha256").update(bytes).digest("hex"),
            browser_download_url:
              "https://github.com/sushi-killer/sushiAI/releases/download/v0.0.2-alpha.1/" +
              name,
          },
        ],
      },
    ];
    globalThis.smokeOpened = [];
    globalThis.smokeInstalled = 0;
    globalThis.smokeUpdates = new Updates({
      directory: root + "/artifacts/update-smoke-cache",
      currentVersion: "0.0.1-alpha.1",
      arch: "arm64",
      automatic: false,
      installer: async () => {
        globalThis.smokeInstalled++;
      },
      fetcher: async (url) =>
        url.includes("api.github.com")
          ? Response.json(releases)
          : new Response(bytes),
      openPath: async (file) => {
        globalThis.smokeOpened.push(file);
        return "";
      },
      openExternal: async () => {},
      onChange: (state) =>
        BrowserWindow.getAllWindows()[0].webContents.send(
          "updates-state",
          state,
        ),
    });
    for (const [channel, method] of [
      ["updates-state", "snapshot"],
      ["updates-check", "check"],
      ["updates-download", "download"],
      ["updates-configure", "configure"],
      ["updates-open", "openInstaller"],
      ["updates-install", "install"],
    ]) {
      ipcMain.removeHandler(channel);
      ipcMain.handle(channel, (_, ...args) =>
        globalThis.smokeUpdates[method](...args),
      );
    }
  }, path.resolve("."));
  await app.evaluate(async () => {
    await globalThis.smokeUpdates.init();
    await globalThis.smokeUpdates.configure({ autoDownload: false });
  });
  await page.evaluate(() =>
    localStorage.setItem(
      "sushiai.v1",
      JSON.stringify({
        activeId: "review",
        socket: "/tmp/absent-sushiai-review.sock",
        routines: [],
        fontScale: 1,
        workspaces: [
          {
            id: "review",
            name: "Review",
            cwd: "/tmp",
            panels: [{ id: "chat", kind: "chat", title: "Thread" }],
            layout: { type: "leaf", id: "chat" },
          },
        ],
      }),
    ),
  );
  await page.reload();
  await page.getByRole("button", { name: /^Dashboard/ }).click();
  await page
    .getByRole("heading", { name: "Pick up where you left off" })
    .waitFor();
  assert.equal(await page.locator(".stat-grid > div").count(), 3);
  assert.equal(
    await page.getByRole("heading", { name: "Working agents" }).count(),
    0,
  );
  if (!(await page.locator(".sidebar").count()))
    await page.keyboard.press("Meta+b");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "0.0.2-alpha.1 is available" })
    .waitFor();
  await page.getByRole("button", { name: "Close dialog", exact: true }).click();
  await page.getByRole("button", { name: "Software update available" }).click();
  await page.screenshot({
    path: path.join(artifacts, "updates-available.png"),
  });
  await page
    .getByRole("button", { name: "Download update", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "0.0.2-alpha.1 is ready to install" })
    .waitFor();
  await page
    .getByRole("button", { name: "Open installer", exact: true })
    .click();
  assert.equal(await app.evaluate(() => globalThis.smokeOpened.length), 1);
  await page.getByRole("checkbox", { name: /Include alpha/ }).click();
  await page.waitForFunction(
    async () =>
      !(await window.bridge.updatesState()).settings.includePrereleases,
  );
  await page
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await page.getByRole("heading", { name: "You’re up to date" }).waitFor();
  await page.getByRole("checkbox", { name: /Include alpha/ }).click();
  await page.waitForFunction(
    async () =>
      (await window.bridge.updatesState()).settings.includePrereleases,
  );
  await page.getByRole("checkbox", { name: /Download automatically/ }).click();
  await page.waitForFunction(
    async () => (await window.bridge.updatesState()).settings.autoDownload,
  );
  await page
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "0.0.2-alpha.1 is ready to install" })
    .waitFor();
  await app.evaluate(() => globalThis.smokeUpdates.emit({ canInstall: true }));
  await page
    .getByRole("button", { name: "Install and restart", exact: true })
    .waitFor();
  await page.screenshot({ path: path.join(artifacts, "updates-ready.png") });
  assert.equal(
    await page
      .getByRole("dialog")
      .evaluate((el) => el.scrollWidth <= el.clientWidth),
    true,
  );
  await app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].setSize(600, 440),
  );
  await page.waitForFunction(() => window.innerHeight === 440);
  assert.equal(
    await page
      .getByRole("dialog")
      .evaluate((el) => el.scrollWidth <= el.clientWidth),
    true,
  );
  await page.screenshot({ path: path.join(artifacts, "updates-compact.png") });
  await page
    .getByRole("button", { name: "Install and restart", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "Preparing update and restart…" })
    .waitFor();
  assert.equal(await app.evaluate(() => globalThis.smokeInstalled), 1);
  assert.deepEqual(errors, []);
  console.log(
    "PASS: original Dashboard restored; update discovery, download, verified installer, channels and preferences.",
  );
} finally {
  await app.close();
  await fs.rm(profile, { recursive: true, force: true });
  await fs.rm(path.join(artifacts, "update-smoke-cache"), {
    recursive: true,
    force: true,
  });
}
