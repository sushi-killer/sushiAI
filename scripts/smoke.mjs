import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import http from "node:http";
import { readdir, stat } from "node:fs/promises";
import { createRequire } from "node:module";
const root = process.cwd();
// Every label, id and storage key below is read out of the fixture, so the
// smoke asserts the contract rather than whichever extension wrote it.
const { validateExtensionManifest } = createRequire(import.meta.url)(
  "../electron/extensions/manifest.cjs",
);
// Validated, not raw: the smoke then reads the same defaults the app does
// instead of restating the contract's fallback rules here.
const probe = validateExtensionManifest({
  ...JSON.parse(
    await fs.readFile("tests/fixtures/extensions/probe/manifest.json", "utf8"),
  ),
  source: {
    kind: "local",
    path: path.join(root, "tests/fixtures/extensions/probe"),
  },
});
// A leftover fixture directory (e.g. a deleted extension's folder someone
// forgot to remove) inflates the extension count the app loads without
// changing anything this smoke reads by id - it only shows up as a mismatched
// count elsewhere. Catch it here instead.
const fixtureDirs = (
  await readdir(path.join(root, "tests/fixtures/extensions"), {
    withFileTypes: true,
  })
)
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();
assert.deepEqual(
  fixtureDirs,
  ["probe"],
  "tests/fixtures/extensions holds an unexpected directory - delete a stale fixture or update this list",
);

const contributed = (kind, id) =>
  probe.contributions[kind].find((item) => item.id === id);
const surfaceOf = (id) => contributed("surfaces", id);
const navLabel = (id) => contributed("navigation", id).label;
const ledger = surfaceOf("probe.ledger");
const itemLabel = ledger.view.document.itemLabel;
const profile = await fs.mkdtemp("/tmp/sushiai-smoke-");
await fs.mkdir("artifacts", { recursive: true });
// The app under test is the built bundle, not the sources: a stale dist silently
// tests the previous build. Fail loudly instead.
const newestSource = (
  await Promise.all(
    (await readdir(path.join(root, "src"), { recursive: true }))
      .filter((name) => /\.(ts|tsx|css)$/.test(name))
      .map((name) => stat(path.join(root, "src", name)).then((s) => s.mtimeMs)),
  )
).reduce((a, b) => Math.max(a, b), 0);
const builtAt = await stat(path.join(root, "dist/index.html")).then(
  (s) => s.mtimeMs,
  () => 0,
);
assert.ok(
  builtAt > newestSource,
  "dist is older than src - run `npm run build` before the desktop smoke",
);

const desktop = await electron.launch({
  ...(process.env.SUSHIAI_EXECUTABLE
    ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
    : {}),
  args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
  cwd: root,
  env: {
    ...process.env,
    BRIDGE_DATA_DIR: profile,
    SUSHIAI_EXTENSIONS_DIR: "tests/fixtures/extensions",
  },
});
const errors = [];
const skipped = [];
const preview = http.createServer((_, response) => {
  response.writeHead(200, { "Content-Type": "text/html" });
  response.end(
    "<!doctype html><title>Bridge preview test</title><h1>LOCAL_PREVIEW_OK</h1>",
  );
});
await new Promise((resolve) => preview.listen(0, "127.0.0.1", resolve));
try {
  const page = await desktop.firstWindow();
  page.on("pageerror", (error) => {
    errors.push(error.message);
    console.error("Renderer:", error.stack);
  });
  await page.waitForSelector(".panel-agent");
  await page
    .waitForFunction(
      () => document.querySelectorAll(".workspace-name").length > 1,
      { timeout: 15000 },
    )
    .catch(() => {});
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  const terminalId = await page
    .locator(".panel-terminal")
    .getAttribute("data-panel-id");
  await page.waitForTimeout(1000);
  await page.evaluate(async (id) => {
    await window.bridge.terminalWrite(id, "printf 'BRIDGE_PTY_OK\\n'\r");
  }, terminalId);
  await page.waitForTimeout(700);
  const output = await page.evaluate(
    async (id) =>
      (
        await window.bridge.terminalOpen({
          panelId: id,
          cwd: (await window.bridge.system()).cwd,
        })
      ).history,
    terminalId,
  );
  assert.ok(
    output.includes("\r\nBRIDGE_PTY_OK"),
    "PTY must execute a real shell command",
  );
  // Herdr is a separate daemon: it is always there on a developer machine but
  // never on a clean CI runner. Skip explicitly rather than silently, and let
  // SUSHIAI_SMOKE_STRICT=1 turn a skip back into a failure.
  const herdr = await page.evaluate(async () => {
    const info = await window.bridge.system();
    try {
      return {
        ok: true,
        result: await window.bridge.herdr(info.socketPath, "ping"),
      };
    } catch (error) {
      return { ok: false, reason: String(error?.message || error) };
    }
  });
  if (herdr.ok) assert.equal(herdr.result.type, "pong");
  else if (process.env.SUSHIAI_SMOKE_STRICT === "1")
    assert.fail(`Herdr is required in strict mode: ${herdr.reason}`);
  else skipped.push(`Herdr ping + workspace sync (${herdr.reason})`);
  assert.equal(await page.title(), "sushiAI");
  assert.ok(
    await page
      .locator(".brand img")
      .evaluate((img) => img.complete && img.naturalWidth > 0),
  );
  assert.equal(
    await page
      .locator("body")
      .innerText()
      .then((text) => text.includes("BridgeMind")),
    false,
  );
  await page.screenshot({ path: path.join(root, "artifacts/workspace.png") });
  await page.getByRole("button", { name: "Tidy", exact: true }).click();
  await page.getByRole("button", { name: "Maximize zsh", exact: true }).click();
  await page.waitForTimeout(300);
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 1);
  await page.keyboard.press("Escape");
  // Wait for the un-zoomed layout to paint rather than guessing a delay.
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 4,
  );
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  const initialAgentBounds = await page.locator(".panel-agent").boundingBox();
  await page
    .locator(".panel-terminal .panel-header")
    .dragTo(page.locator(".panel-agent"), {
      targetPosition: {
        x: initialAgentBounds.width / 2,
        y: initialAgentBounds.height / 2,
      },
    });
  await page.waitForTimeout(200);
  const movedTerminalBounds = await page
    .locator(".panel-terminal")
    .boundingBox();
  assert.ok(
    Math.abs(movedTerminalBounds.x - initialAgentBounds.x) < 10,
    "Drag should swap panels",
  );
  await page.getByRole("button", { name: "Tidy", exact: true }).click();
  const handle = page.locator(".workspace-canvas > .split > .split-handle.row");
  const handleBounds = await handle.boundingBox();
  await page.waitForTimeout(150);
  const beforeResize = await page.locator(".panel-agent").boundingBox();
  await page.mouse.move(handleBounds.x + 4, handleBounds.y + 80);
  await page.mouse.down();
  await page.mouse.move(handleBounds.x + 64, handleBounds.y + 80, { steps: 8 });
  await page.mouse.up();
  const afterResize = await page.locator(".panel-agent").boundingBox();
  console.log("Resize evidence", { handleBounds, beforeResize, afterResize });
  assert.ok(
    afterResize.width > beforeResize.width + 30,
    "Divider must resize the pane",
  );
  await page
    .getByRole("textbox", { name: "Browser address" })
    .fill(`http://127.0.0.1:${preview.address().port}`);
  await page.getByRole("textbox", { name: "Browser address" }).press("Enter");
  await page.waitForTimeout(800);
  const browserResult = await desktop.evaluate(async ({ webContents }) => {
    const guest = webContents
      .getAllWebContents()
      .find((contents) => contents.getType() === "webview");
    return guest.executeJavaScript(
      "({ text: document.body.innerText, bridge: typeof window.bridge, node: typeof process })",
    );
  });
  assert.ok(browserResult.text.includes("LOCAL_PREVIEW_OK"));
  assert.equal(browserResult.bridge, "undefined");
  assert.equal(browserResult.node, "undefined");
  await page.keyboard.press("Meta+k");
  await page
    .getByRole("button", {
      name: "Terminal A real shell in your project",
      exact: false,
    })
    .click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 5);
  await page
    .getByRole("button", { name: "Close zsh", exact: true })
    .last()
    .click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page.getByText(probe.name, { exact: true }).first().waitFor();
  assert.equal(
    await page
      .locator(".extension-card")
      .filter({ hasText: probe.name })
      .count(),
    1,
    "the fixture is the only extension the suite installs beside the built-ins",
  );
  assert.equal(
    await page
      .getByRole("button", { name: navLabel("probe.nav-mode"), exact: true })
      .count(),
    0,
  );
  await page
    .getByRole("button", { name: `Enable ${probe.name}`, exact: true })
    .click();
  // An entry beside Agent/Code/Chat still opens a page in the Code shell, and
  // marks only itself: two lit buttons in one segmented control would read as
  // two things selected at once.
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Chat", exact: true })
    .click();
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: navLabel("probe.nav-mode"), exact: true })
    .click();
  await page.locator(".extension-surface").waitFor({ state: "visible" });
  await page.locator(".primary-nav").waitFor({ state: "visible" });
  assert.equal(
    await page.locator(".sidebar-slot").count(),
    0,
    "opening a page from Chat must not leave the sidebar blank",
  );
  assert.ok(
    await page.getByTitle("Files and Git").count(),
    "the workspace toolbar belongs to a page, whoever contributed it",
  );
  assert.deepEqual(
    await page.locator(".mode-switch button.active").allTextContents(),
    [navLabel("probe.nav-mode")],
    "exactly one control in the mode switch is selected",
  );
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Code", exact: true })
    .click();
  // An extension pane is added from the ordinary add-panel list, because the
  // manifest asked for a spot there - not because Extensions offers a button.
  await page.keyboard.press("Meta+k");
  await page
    .getByRole("button", {
      name: navLabel("probe.nav-picker-table"),
      exact: false,
    })
    .click();
  await page.locator(".panel-extension").waitFor({ state: "visible" });
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 5);
  // Picking a singleton that is already on screen brings it forward instead of
  // opening a second one. Whether "multiple" adds another is a question about
  // resolvePaneOpen, and a unit test answers it without a window.
  await page.keyboard.press("Meta+k");
  await page
    .getByRole("button", {
      name: navLabel("probe.nav-picker-table"),
      exact: false,
    })
    .click();
  await page.waitForTimeout(250);
  assert.equal(
    await page.locator(".workspace-canvas .panel").count(),
    1,
    "a singleton already on screen is brought forward, not opened twice",
  );
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Code", exact: true })
    .click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 5);
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page
    .getByRole("button", { name: `Disable ${probe.name}`, exact: true })
    .click();
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Code", exact: true })
    .click();
  await page.locator(".workspace-name.active").click();
  await page.locator(".extension-unavailable").waitFor({ state: "visible" });
  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page
    .getByRole("button", { name: `Enable ${probe.name}`, exact: true })
    .click();
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Code", exact: true })
    .click();
  await page.locator(".workspace-name.active").click();
  await page.locator(".extension-surface").waitFor({ state: "visible" });
  await page.getByTitle("Files and Git").click();
  await page.getByRole("button", { name: "History", exact: true }).click();
  await page.locator(".git-history").waitFor({ state: "visible" });
  await page.locator(".history-commit").first().waitFor({ state: "visible" });
  assert.equal(
    await page.locator(".git-history-list .history-error").count(),
    0,
  );
  await page
    .getByRole("button", { name: "Close Files & Git", exact: true })
    .click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 5);
  // The pane opened earlier is still here; closing it returns the workspace
  // to the four panels it started with.
  // The contribution contract is live: a sidebar entry, an embedded dashboard
  // section and a per-workspace folder action all come from the same manifest.
  await page
    .locator(".primary-nav")
    .getByRole("button", {
      name: navLabel("probe.nav-sidebar-ledger"),
      exact: true,
    })
    .waitFor({ state: "visible" });
  // The Dashboard entry carries a workspace counter, so its name is not exact.
  await page.locator(".primary-nav").getByText("Dashboard").click();
  await page.locator(".extension-section").waitFor({ state: "visible" });
  assert.ok(
    (await page.locator(".extension-section .extension-surface").count()) > 0,
    "an extension section renders through the host-owned safe renderer",
  );
  assert.ok(
    await page.locator(".extension-section .extension-layout-table").count(),
    "a surface hosted in a section draws the layout it declared",
  );
  // Every toolbar spot the contract names is a real spot: a manifest that asks
  // for one gets a button there, not silently nothing.
  for (const action of probe.contributions.actions.filter((item) =>
    item.defaultPlacement.startsWith("workspace.toolbar."),
  ))
    assert.ok(
      await page
        .getByRole("button", { name: action.label, exact: true })
        .count(),
      `${action.defaultPlacement} renders its contributed action`,
    );
  await page.getByRole("button", { name: "Skills", exact: true }).click();
  await page.locator(".extension-section").waitFor({ state: "visible" });
  assert.ok(
    await page.locator(".extension-section .extension-layout-board").count(),
    "skills.section hosts a surface the same way dashboard.section does",
  );
  await page.locator(".primary-nav").getByText("Dashboard").click();
  assert.equal(
    await page
      .getByRole("button", {
        name: contributed("actions", "probe.act-folder").label,
        exact: true,
      })
      .count(),
    await page.locator(".workspace-item").count(),
    "each workspace row carries the contributed folder action",
  );
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: "Code", exact: true })
    .click();
  // A surface must save only what the user changed. Loading used to count as a
  // change, so each save was announced, re-read and saved again forever.
  const stateFile = path.join(profile, `extensions/state/${probe.id}.json`);
  const typed = `smoke-${Date.now()}`;
  // The compose box belongs to the surface people edit, so the edit is made on
  // the owner page; every other view of that slice is read-only by contract.
  await page
    .locator(".mode-switch")
    .getByRole("button", { name: navLabel("probe.nav-mode"), exact: true })
    .click();
  await page.locator(".section-page .extension-surface").waitFor();
  await page.getByRole("textbox", { name: `New ${itemLabel}` }).fill(typed);
  await page
    .getByRole("button", { name: `Add ${itemLabel}`, exact: true })
    .click();
  await page.waitForFunction(
    (text) => document.body.innerText.includes(text),
    typed,
  );
  await page.waitForTimeout(600);
  const settled = await fs.stat(stateFile).then((info) => info.mtimeMs);
  await page.waitForTimeout(1500);
  assert.equal(
    await fs.stat(stateFile).then((info) => info.mtimeMs),
    settled,
    "an idle surface must stop writing once its edit has been saved",
  );
  assert.ok(
    JSON.parse(await fs.readFile(stateFile, "utf8"))[ledger.stateId][
      String(ledger.stateVersion)
    ],
    `the ${itemLabel} was stored under its state slice and version`,
  );
  // A contributed entry with no order of its own sorts after every built-in
  // section rather than claiming the top of the host's own navigation.
  assert.deepEqual(
    (await page.locator(".primary-nav > *").allTextContents()).map((label) =>
      label.replace(/\d+$/, ""),
    ),
    [
      "Dashboard",
      "Sessions",
      "Routines",
      "Extensions",
      "Skills",
      ...probe.contributions.navigation
        .filter((item) => item.defaultPlacement === "sidebar.primary")
        .sort((a, b) => a.order - b.order || a.id.localeCompare(b.id))
        .map((item) => item.label),
    ],
    "a section of the app always comes first; contributed entries follow in the order the registry gives every placement",
  );
  await page
    .locator(".primary-nav")
    .getByRole("button", {
      name: navLabel("probe.nav-sidebar-board"),
      exact: true,
    })
    .click();
  await page.locator(".extension-layout-board").waitFor({ state: "visible" });
  // A contributed page opens the way Skills does. It may not take the frame:
  // the nav, the workspace list and the toolbar all stay, and the entry that
  // opened it is marked, or nothing in the sidebar says where you are.
  await page.locator(".primary-nav").waitFor({ state: "visible" });
  await page.locator(".workspace-section").waitFor({ state: "visible" });
  assert.equal(
    await page.locator(".mode-switch button.active").textContent(),
    "Code",
  );
  assert.ok(
    await page.getByTitle("Files and Git").count(),
    "the workspace toolbar stays while a contributed page is open",
  );
  assert.equal(
    await page.locator(".primary-nav .nav-item.current").textContent(),
    navLabel("probe.nav-sidebar-board"),
  );
  assert.equal(
    await page.locator(".sidebar-slot").count(),
    0,
    "the sidebar is the Code sidebar, not the blank slot Agent and Chat fill",
  );
  assert.ok(
    await page.locator(".section-page .extension-surface").count(),
    "the page is drawn in the same frame a core section is",
  );
  // The board draws its declared columns before the projects are read, so it
  // is visible while still empty: wait for the record, not for the frame.
  await page
    .locator(".extension-group")
    .filter({ hasText: typed })
    .first()
    .waitFor({ state: "visible" });
  assert.equal(
    await page.locator(".extension-surface-compose").count(),
    0,
    "an aggregate has no project to add to, so it offers no compose box",
  );
  await page.locator(".workspace-name").first().click();
  await page
    .getByRole("button", {
      name: `Close ${surfaceOf("probe.table").title}`,
      exact: true,
    })
    .click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  assert.equal(await page.locator(".panel-extension").count(), 0);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.waitFor();
  await settings.getByRole("tab", { name: "Connections", exact: true }).click();
  // Same Herdr-availability gate as the ping check above: this text only
  // ever renders once a real Herdr daemon accepts the connection, which a
  // clean CI runner never has.
  if (herdr.ok)
    await page
      .getByText("Connected · workspaces sync automatically")
      .waitFor({ state: "visible" });
  else skipped.push(`Connections tab "Connected" state (${herdr.reason})`);
  await settings.getByRole("tab", { name: "Providers", exact: true }).click();
  await settings.locator(".providers-settings").waitFor({ state: "visible" });
  await settings.getByRole("tab", { name: "Updates", exact: true }).click();
  await settings.locator(".update-settings").waitFor({ state: "visible" });
  await page.getByRole("button", { name: "Close dialog" }).click();
  await page.getByRole("button", { name: "Routines", exact: true }).click();
  await page.getByRole("button", { name: "New routine", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Name", exact: true })
    .fill("Smoke routine");
  await page
    .getByRole("textbox", { name: "Command", exact: true })
    .fill("printf 'ROUTINE_OK\\n'");
  await page.getByRole("button", { name: "Save routine" }).click();
  await page.getByRole("button", { name: "Run routine", exact: true }).click();
  await page.waitForTimeout(500);
  const routineId = await page
    .locator(".workspace-canvas .panel")
    .getAttribute("data-panel-id");
  const routineOutput = await page.evaluate(
    async (id) =>
      (
        await window.bridge.terminalOpen({
          panelId: id,
          cwd: (await window.bridge.system()).cwd,
        })
      ).history,
    routineId,
  );
  assert.ok(routineOutput.includes("\r\nROUTINE_OK"));
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Close Smoke routine" }).click();
  await page.waitForTimeout(700);
  await page.reload();
  await page.waitForSelector(".panel-agent");
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  assert.deepEqual(errors, [], "No uncaught renderer errors");
  console.log(
    JSON.stringify(
      {
        passed: true,
        checks: [
          "native window",
          "real PTY command",
          "Tidy and maximize",
          "drag and drop",
          "divider resize",
          "real embedded browser + Node/IPC isolation",
          "add and close terminal",
          "external-style extension panel and disable/restore",
          "extension contribution contract: nav, section and folder action",
          "extension state saves the edit and then stops writing",
          "a contributed page opens like Skills: sidebar, toolbar and Code stay",
          "settings connection",
          "routine execution",
          "layout persistence",
          "no renderer errors",
          ...(skipped.length ? [] : ["Herdr ping + workspace sync"]),
        ],
        skipped,
        screenshot: "artifacts/workspace.png",
      },
      null,
      2,
    ),
  );
} catch (error) {
  const page = await desktop.firstWindow();
  await page.screenshot({ path: path.join(root, "artifacts/failure.png") });
  console.error("Renderer errors:", errors);
  throw error;
} finally {
  await new Promise((resolve) => preview.close(resolve));
  await desktop.close();
  await fs.rm(profile, { recursive: true, force: true });
}
