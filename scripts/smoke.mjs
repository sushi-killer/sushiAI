import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import http from "node:http";
const root = process.cwd();
const profile = await fs.mkdtemp("/tmp/sushiai-smoke-");
await fs.mkdir("artifacts", { recursive: true });
const desktop = await electron.launch({
  ...(process.env.SUSHIAI_EXECUTABLE
    ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
    : {}),
  args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
  cwd: root,
  env: { ...process.env, BRIDGE_DATA_DIR: profile },
});
const errors = [];
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
  const result = await page.evaluate(async () => {
    const info = await window.bridge.system();
    return window.bridge.herdr(info.socketPath, "ping");
  });
  assert.equal(result.type, "pong");
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
  await page.waitForTimeout(300);
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
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("dialog", { name: "Settings" }).waitFor();
  assert.ok(
    await page
      .getByText("Connected · workspaces sync automatically")
      .isVisible(),
  );
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
          "Herdr ping + workspace sync",
          "Tidy and maximize",
          "drag and drop",
          "divider resize",
          "real embedded browser + Node/IPC isolation",
          "add and close terminal",
          "settings connection",
          "routine execution",
          "layout persistence",
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
  await desktop.close();
  await fs.rm(profile, { recursive: true, force: true });
}
