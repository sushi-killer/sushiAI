import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import http from "node:http";
import { readdir, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { daemonBinary, stopDaemon } from "./lib/daemon-binary.mjs";
const root = process.cwd();

// Polls a check instead of sleeping a fixed time: run straight after
// `npm run ci`, a loaded machine missed 200-500ms sleeps on checks that pass
// on rerun. Returns the last value read, so the assert after it still names
// what never became true.
async function until(read, ready, timeout = 10000) {
  const deadline = Date.now() + timeout;
  let value = await read();
  while (!ready(value) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    value = await read();
  }
  return value;
}
// Every label, id and storage key below is read out of the fixture, so the
// smoke asserts the contract rather than whichever extension wrote it.
const { readSnapshot } = createRequire(import.meta.url)(
  "../electron/workspace-snapshot.cjs",
);
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
await fs.writeFile(path.join(profile, ".zshrc"), "");
// The strict daemon: the app under test starts its own `sushiai` daemon in a
// home of its own and never touches the owner's ~/.sushiai.
const daemonHome = path.join(profile, "sushiai");
const daemonPid = async () =>
  Number(
    (await fs.readFile(path.join(daemonHome, "daemon.lock"), "utf8")).trim(),
  );
const orchdBuilt = existsSync(path.join(root, "orchd/target/release/orchd"));
// Polls `ps` until a daemon for this profile's data dir is (or is no longer)
// running; true when the wanted state was reached in time.
async function pollOrchd(wantRunning, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  do {
    const running = execFileSync("ps", ["-axo", "pid=,command="], {
      encoding: "utf8",
    })
      .split("\n")
      .some((line) => line.includes(`--data ${profile}/orchestrator`));
    if (running === wantRunning) return true;
    await new Promise((resolve) => setTimeout(resolve, 200));
  } while (Date.now() < deadline);
  return false;
}
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

const launchApp = () =>
  electron.launch({
    ...(process.env.SUSHIAI_EXECUTABLE
      ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
      : {}),
    args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
    cwd: root,
    env: {
      ...process.env,
      BRIDGE_DATA_DIR: profile,
      ZDOTDIR: profile,
      SUSHIAI_HOME: daemonHome,
      SUSHIAI_DAEMON_BIN: process.env.SUSHIAI_DAEMON_BIN || daemonBinary(root),
      SUSHIAI_EXTENSIONS_DIR: "tests/fixtures/extensions",
      // Inherited from `npm run dev`, it would load the dev server, not dist/.
      BRIDGE_DEV_URL: "",
      // The smoke drives a real Electron app: keep its window off screen so a
      // test run never steals focus or covers what you are working in.
      SUSHIAI_TEST_WINDOW: "hidden",
    },
  });
let desktop = await launchApp();
// A hidden run must put nothing on the owner's screen and still render at the
// real content size, so screenshots and measurements can be trusted.
async function assertHiddenWindow(page) {
  const state = await desktop.evaluate(({ BrowserWindow, app }) => {
    const [win] = BrowserWindow.getAllWindows();
    const [width, height] = win.getContentSize();
    return {
      visible: BrowserWindow.getAllWindows().filter((w) => w.isVisible())
        .length,
      focused: BrowserWindow.getFocusedWindow() !== null,
      dockVisible: process.platform === "darwin" ? app.dock.isVisible() : false,
      content: { width, height },
    };
  });
  assert.equal(state.visible, 0, "no BrowserWindow may be visible");
  assert.equal(state.focused, false, "no BrowserWindow may be focused");
  assert.equal(state.dockVisible, false, "the dock icon must be hidden");
  const viewport = await page.evaluate(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
    ratio: window.devicePixelRatio,
  }));
  assert.equal(viewport.width, state.content.width);
  assert.equal(viewport.height, state.content.height);
  assert.deepEqual(
    [viewport.width, viewport.height],
    [1380, 880],
    "viewport is the 1380x880 content size",
  );
  const file = path.join(root, "artifacts/smoke-hidden-window.png");
  await page.screenshot({ path: file });
  const png = await fs.readFile(file);
  assert.equal(png.readUInt32BE(16), viewport.width * viewport.ratio);
  assert.equal(png.readUInt32BE(20), viewport.height * viewport.ratio);
  console.log(
    `Hidden window: ${state.visible} visible, none focused, dock hidden, viewport ${viewport.width}x${viewport.height}`,
  );
}

const errors = [];
const preview = http.createServer((_, response) => {
  response.writeHead(200, { "Content-Type": "text/html" });
  response.end(
    "<!doctype html><title>Bridge preview test</title><h1>LOCAL_PREVIEW_OK</h1>",
  );
});
await new Promise((resolve) => preview.listen(0, "127.0.0.1", resolve));
try {
  let page = await desktop.firstWindow();
  page.on("pageerror", (error) => {
    errors.push(error.message);
    console.error("Renderer:", error.stack);
  });
  await page.waitForSelector(".panel-agent");
  await assertHiddenWindow(page);
  await page
    .waitForFunction(
      () => document.querySelectorAll(".workspace-name").length > 1,
      { timeout: 15000 },
    )
    .catch(() => {});
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  // 1. The local daemon is ready.
  const daemonState = () =>
    page.evaluate(async () =>
      (await window.bridge.daemonStates()).find((s) => s.host === "local"),
    );
  const ready = await until(
    daemonState,
    (state) => state?.state === "ready",
    30000,
  );
  assert.equal(ready?.state, "ready", `local daemon: ${JSON.stringify(ready)}`);
  // 2. A shell panel runs a real command inside a daemon session.
  await page
    .locator(".panel-terminal")
    .getByRole("button", { name: /^Launch / })
    .click();
  await page.locator(".panel-terminal .xterm").waitFor();
  const terminalId = await page
    .locator(".panel-terminal")
    .getAttribute("data-panel-id");
  const sessionId = await until(
    () =>
      page.evaluate(
        async () =>
          (await window.bridge.sessionsList("local")).find(
            (session) => session.status === "running",
          )?.id,
      ),
    Boolean,
  );
  assert.ok(sessionId, "launching the shell panel starts a daemon session");
  const screen = (id = sessionId) =>
    page.evaluate(
      async (value) =>
        (await window.bridge.sessionRead("local", value, 200)).text,
      id,
    );
  const type = (text) =>
    page.evaluate(
      ({ id, data }) => window.bridge.daemonTerminalWrite(id, data),
      { id: terminalId, data: text },
    );
  await type("printf 'BRIDGE_PTY_OK\\n'\r");
  const echoed = await until(screen, (text) => /^BRIDGE_PTY_OK$/m.test(text));
  assert.match(
    echoed,
    /^BRIDGE_PTY_OK$/m,
    `the shell must run a real command; screen: ${JSON.stringify(echoed)}`,
  );
  // 3. kill -9 of the daemon: the session lives in its holder, a new daemon
  // adopts it and the panel shows the same screen.
  // The prompt comes after the command's output: wait until the screen rests.
  const restingScreen = async () => {
    let last = await screen();
    for (let i = 0; i < 20; i++) {
      await new Promise((resolve) => setTimeout(resolve, 600));
      const next = await screen();
      if (next === last) return next;
      last = next;
    }
    return last;
  };
  const before = (await restingScreen()).trimEnd();
  const oldDaemon = await daemonPid();
  const generation = (await daemonState()).generation;
  process.kill(oldDaemon, "SIGKILL");
  const back = await until(
    daemonState,
    (state) => state?.state === "ready" && state.generation > generation,
    30000,
  );
  assert.equal(back?.state, "ready", "the app starts a new daemon");
  assert.notEqual(await daemonPid(), oldDaemon);
  const after = await until(
    async () => (await screen()).trimEnd(),
    (text) => text === before,
  );
  assert.equal(after, before, "the same screen after the daemon died");
  await page.waitForFunction(() =>
    document
      .querySelector(".panel-terminal .xterm-rows")
      ?.textContent.includes("BRIDGE_PTY_OK"),
  );
  // 4. A resize reaches the program (SIGWINCH makes a TUI redraw).
  await type(
    "sh -c 'trap \"echo TUI_SIZE \\$(stty size)\" WINCH; while :; do sleep 0.1; done'\r",
  );
  await page.waitForTimeout(500);
  await page.evaluate(
    (id) => window.bridge.daemonTerminalResize(id, 100, 30),
    terminalId,
  );
  const resized = await until(screen, (text) => /TUI_SIZE 30 100/.test(text));
  assert.match(resized, /TUI_SIZE 30 100/, "the program saw the new size");
  await type("\x03");
  await until(screen, (text) => /❯ $/.test(text.trimEnd() + " "));
  // 5. An app restart: the daemon stays, the panel reattaches, the layout is back.
  const layoutOf = async () => {
    const saved = JSON.parse(readSnapshot(profile) ?? "null");
    const active = saved?.workspaces.find((w) => w.id === saved.activeId);
    return active && active.panels.some((p) => p.sessionId === sessionId)
      ? JSON.stringify(active.layout)
      : null;
  };
  const layout = await until(layoutOf, Boolean);
  assert.ok(layout, "the bound session and the layout are saved");
  const keptDaemon = await daemonPid();
  await desktop.close();
  desktop = await launchApp();
  page = await desktop.firstWindow();
  page.on("pageerror", (error) => {
    errors.push(error.message);
    console.error("Renderer:", error.stack);
  });
  await page.waitForSelector(".panel-agent");
  await assertHiddenWindow(page);
  assert.equal(await daemonPid(), keptDaemon, "the daemon outlived the app");
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  assert.equal(await layoutOf(), layout, "the layout is back after a restart");
  await page.waitForFunction(
    () =>
      document
        .querySelector(".panel-terminal .xterm-rows")
        ?.textContent.includes("BRIDGE_PTY_OK"),
    null,
    { timeout: 30000 },
  );
  assert.equal(
    await page.locator(".terminal-ended").count(),
    0,
    "the session is reattached, not ended",
  );
  // 6. Closing a live session asks first; cancelling keeps it running.
  // The session names its panel (the folder), so the shell is found by its
  // place in the workspace, not by a fixed title.
  const shellTitle = (
    await page
      .locator(".panel-terminal")
      .first()
      .getByRole("button", { name: /^Close / })
      .getAttribute("aria-label")
  ).replace(/^Close /, "");
  await page
    .getByRole("button", { name: `Close ${shellTitle}`, exact: true })
    .click();
  await page
    .getByRole("button", { name: "Close session", exact: true })
    .waitFor();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  const stillRunning = await page.evaluate(
    async (id) =>
      (await window.bridge.sessionsList("local")).find((s) => s.id === id)
        ?.status,
    sessionId,
  );
  assert.equal(stillRunning, "running", "Cancel leaves the session running");
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
  await page
    .getByRole("button", { name: `Maximize ${shellTitle}`, exact: true })
    .click();
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 1,
  );
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
  await page
    .waitForFunction(
      (x) => {
        const terminal = document.querySelector(".panel-terminal");
        return (
          !!terminal && Math.abs(terminal.getBoundingClientRect().x - x) < 10
        );
      },
      initialAgentBounds.x,
      { timeout: 10000 },
    )
    .catch(() => {});
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
  const browserResult = await until(
    () =>
      desktop.evaluate(async ({ webContents }) => {
        const guest = webContents
          .getAllWebContents()
          .find((contents) => contents.getType() === "webview");
        return guest?.executeJavaScript(
          "({ text: document.body.innerText, bridge: typeof window.bridge, node: typeof process })",
        );
      }),
    (result) => Boolean(result?.text?.includes("LOCAL_PREVIEW_OK")),
  );
  assert.ok(browserResult.text.includes("LOCAL_PREVIEW_OK"));
  assert.equal(browserResult.bridge, "undefined");
  assert.equal(browserResult.node, "undefined");
  await page.keyboard.press("Meta+k");
  await page
    .getByRole("button", {
      name: "Terminal A shell in the project",
      exact: false,
    })
    .click();
  // The new shell is a daemon session: its panel appears once it is launched.
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 5,
  );
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 5);
  await page
    .getByRole("button", { name: `Close ${shellTitle}`, exact: true })
    .last()
    .click();
  await page
    .getByRole("button", { name: "Close session", exact: true })
    .click();
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 4,
  );
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  // The Inbox replaced the Sessions dialog: it lists the same panels as an
  // attention queue, and Jump gets you back to the workspace the same way
  // the old dialog's Show did. Its nav entry may carry a waiting count next
  // to the label (real agents elsewhere on this machine can be blocked or
  // done), so the click below matches on the label alone.
  await page
    .locator(".primary-nav")
    .getByRole("button", { name: "Inbox", exact: false })
    .click();
  await page
    .locator(".section-page")
    .getByRole("heading", { name: "Inbox", exact: true })
    .waitFor();
  // A plain shell never sits in the queue: it is listed for review under
  // Clean up, unticked (it may be running a dev server), with its own Jump.
  await page
    .locator(".section-page")
    .getByRole("button", { name: "Clean up", exact: true })
    .click();
  const shellRow = page.locator(".inbox-cleanup-row").filter({
    has: page.getByRole("checkbox", {
      name: "End zsh in sushiai",
      exact: true,
    }),
  });
  await shellRow.waitFor();
  assert.equal(
    await shellRow.getByRole("checkbox").isChecked(),
    false,
    "Clean up never ticks a shell by default",
  );
  await shellRow.getByRole("button", { name: "Jump to zsh" }).click();
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 1,
  );
  assert.equal(
    await page.locator(".section-page").count(),
    0,
    "Jump returns to the workspace canvas, closing the Inbox page",
  );
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 1);
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () => document.querySelectorAll(".workspace-canvas .panel").length === 4,
  );
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
  await page
    .waitForFunction(
      () => document.querySelectorAll(".workspace-canvas .panel").length === 1,
      null,
      { timeout: 10000 },
    )
    .catch(() => {});
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
  // A second connection sees `data_version` move only when the app commits.
  const stateDb = new DatabaseSync(path.join(profile, "sushiai.db"), {
    readOnly: true,
  });
  const commits = () =>
    stateDb.prepare("PRAGMA data_version").get().data_version;
  const surfaceState = () =>
    JSON.parse(
      stateDb
        .prepare(
          "SELECT value FROM store WHERE name = 'surface-state' AND key = ?",
        )
        .get(probe.id)?.value ?? "null",
    );
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
  // Other work may still commit shortly after the edit (the catalog sync
  // follows the workspace list); a surface that loops never goes quiet.
  let lastCommits = commits();
  let quietSince = Date.now();
  for (
    let waited = 0;
    Date.now() - quietSince < 2000 && waited < 20000;
    waited += 200
  ) {
    await page.waitForTimeout(200);
    const now = commits();
    if (now !== lastCommits) {
      lastCommits = now;
      quietSince = Date.now();
    }
  }
  assert.ok(
    Date.now() - quietSince >= 2000,
    "an idle surface must stop writing once its edit has been saved",
  );
  assert.ok(
    surfaceState()?.[ledger.stateId]?.[String(ledger.stateVersion)],
    `the ${itemLabel} was stored under its state slice and version`,
  );
  // A contributed entry with no order of its own sorts after every built-in
  // section rather than claiming the top of the host's own navigation.
  assert.deepEqual(
    (await page.locator(".primary-nav > *").allTextContents()).map((label) =>
      label.replace(/\d+$/, ""),
    ),
    [
      "Inbox",
      "Routines",
      "Dashboard",
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
  await settings
    .locator(".host-status-line")
    .filter({ hasText: "Connected" })
    .first()
    .waitFor({ state: "visible" });
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
  // runRoutine opens the terminal and writes the command before it adds the
  // panel, so once the panel is on screen its history only has to catch up.
  const routinePanel = page.locator(".workspace-canvas .panel", {
    has: page.getByRole("button", { name: "Close Smoke routine" }),
  });
  await routinePanel.waitFor();
  const routineSession = await until(
    () =>
      page.evaluate(
        async () =>
          (await window.bridge.sessionsList("local")).find((session) =>
            session.title?.includes("Smoke routine"),
          )?.id,
      ),
    Boolean,
  );
  assert.ok(routineSession, "the routine runs in its own daemon session");
  const routineOutput = await until(
    () => screen(routineSession),
    (text) => /^ROUTINE_OK$/m.test(text),
  );
  assert.match(routineOutput, /^ROUTINE_OK$/m);
  // The daemon starts lazily on the first request; send one so the exit check
  // in `finally` has a daemon to look for.
  if (orchdBuilt) await page.evaluate(() => window.bridge.orchestrator("ping"));
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Close Smoke routine" }).click();
  await page
    .getByRole("button", { name: "Close session", exact: true })
    .click();
  // The reload must find the close already persisted, not race its write.
  await until(async () => {
    const saved = JSON.parse(readSnapshot(profile) ?? "null");
    const active = saved?.workspaces.find((w) => w.id === saved.activeId);
    return Boolean(
      active && !active.panels.some((p) => p.title === "Smoke routine"),
    );
  }, Boolean);
  await page.reload();
  await page.waitForSelector(".panel-agent");
  assert.equal(await page.locator(".workspace-canvas .panel").count(), 4);
  await assertHiddenWindow(page);
  // orchd starts lazily on the first real orchestrator request; make one so
  // the teardown below has a daemon to prove it stops with the app.
  if (orchdBuilt) {
    await page.evaluate(() =>
      window.bridge.orchestrator("chat.list", {}).catch(() => null),
    );
  }
  assert.deepEqual(errors, [], "No uncaught renderer errors");
  console.log(
    JSON.stringify(
      {
        passed: true,
        checks: [
          "native window",
          "local daemon ready",
          "real shell command in a daemon session",
          "resize reaches the program",
          "kill -9 of the daemon: same screen after reattach",
          "app restart: reattach and layout",
          "close confirmation for a live session",
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
          "orchd starts on first use and exits with the app",
          "no renderer errors",
        ],
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
  // orchd starts on first use, so make one call. A test-launched app stops
  // its own daemon on quit: prove one ran (so the check below cannot pass
  // trivially), then that none outlives the app.
  if (orchdBuilt)
    await (
      await desktop.firstWindow()
    )
      .evaluate(() => window.bridge.orchestrator("task.list", {}))
      .catch(() => {});
  const orchdRan = orchdBuilt && (await pollOrchd(true, 10000));
  await desktop.close();
  const orchdGone = !orchdBuilt || (await pollOrchd(false, 3000));
  stopDaemon(daemonHome);
  await fs.rm(profile, { recursive: true, force: true });
  assert.ok(!orchdBuilt || orchdRan, "orchd never started for this data dir");
  assert.ok(orchdGone, "orchd for this data dir outlived the app");
}
