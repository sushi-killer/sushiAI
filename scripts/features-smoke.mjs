import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { Connections, run, quote } = require("../electron/connections.cjs");
const { request } = require("../electron/herdr.cjs");
const host = process.env.SUSHIAI_SSH_HOST;
if (!host) throw new Error("Set SUSHIAI_SSH_HOST to your SSH test host.");
const profile = await fs.mkdtemp("/tmp/sushiai-features-");
const connections = new Connections(profile);
await connections.init();
const p = await connections.save({
  name: "Remote test",
  host,
  socket:
    process.env.SUSHIAI_SSH_SOCKET ||
    "~/.config/herdr/sessions/sushiai/herdr.sock",
});
const endpoint = "ssh:" + p.id;
let root, socket, workspace, desktop, page;
const remote = (code) =>
  run("/usr/bin/ssh", [
    ...connections.args(p),
    p.host,
    "python3 -c " + quote(code),
  ]);
try {
  root = (
    await remote(`import tempfile,pathlib,subprocess
r=pathlib.Path(tempfile.mkdtemp(prefix='sushiai-ui-fixture-'))
(r/'hello.txt').write_text('Original line\\n')
(r/'index.html').write_text('<!doctype html><title>Remote demo</title><link rel="stylesheet" href="style.css"><h1>REMOTE_PREVIEW_OK</h1>')
(r/'style.css').write_text('h1{color:rgb(255, 127, 80)}')
(r/'art.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100"><rect width="160" height="100" fill="coral"/></svg>')
subprocess.run(['git','init',str(r)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
subprocess.run(['git','-C',str(r),'add','.'],check=True)
subprocess.run(['git','-C',str(r),'-c','user.name=Demo','-c','user.email=demo@example.invalid','commit','-qm','fixture'],check=True)
(r/'hello.txt').write_text('Updated remote text\\n')
print(r)`)
  ).trim();
  socket = await connections.socket(endpoint);
  const created = await request(socket, "workspace.create", {
    label: "sushiAI feature test",
    cwd: root,
    focus: false,
  });
  workspace = created.workspace.workspace_id;
  const pane = created.root_pane.pane_id;
  desktop = await electron.launch({
    ...(process.env.SUSHIAI_EXECUTABLE
      ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
      : {}),
    args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
    env: {
      ...process.env,
      SUSHIAI_TEST_HEADLESS: "1",
      BRIDGE_DATA_DIR: profile,
    },
  });
  page = await desktop.firstWindow();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.waitForSelector(".panel");
  await page.evaluate(
    ({ endpoint, workspace, pane, root }) => {
      const id = "herdr:" + endpoint + ":" + workspace,
        panelId = "herdr:" + endpoint + ":" + pane;
      localStorage.setItem(
        "sushiai.v1",
        JSON.stringify({
          socket: endpoint,
          activeId: id,
          routines: [],
          fontScale: 1,
          workspaces: [
            {
              id,
              name: "sushiAI feature test",
              herdrId: workspace,
              connection: endpoint,
              cwd: root,
              panels: [
                { id: panelId, herdrId: pane, kind: "terminal", title: "zsh" },
              ],
              layout: { type: "leaf", id: panelId },
            },
          ],
        }),
      );
    },
    { endpoint, workspace, pane, root },
  );
  await page.reload();
  await page.waitForSelector(".herdr-terminal-note");
  await page.waitForTimeout(1200);
  assert.equal(await page.locator(".panel-error").count(), 0);
  await page.getByTitle("Files and Git", { exact: true }).click();
  await page
    .getByRole("button", { name: "Maximize Files & Git", exact: true })
    .click();
  await page.locator('.file-row[title="hello.txt"]').click();
  await page.waitForFunction(() =>
    document
      .querySelector(".file-code")
      ?.textContent.includes("Updated remote text"),
  );
  await page.getByTitle("Edit text file", { exact: true }).click();
  await page
    .getByRole("textbox", { name: "Edit hello.txt", exact: true })
    .fill("Edited over SSH from sushiAI\n");
  await page
    .getByRole("textbox", { name: "Edit hello.txt", exact: true })
    .press("Meta+s");
  await page.getByTitle("Edit text file", { exact: true }).waitFor();
  const savedFile = await connections.inspect(endpoint, {
    operation: "read",
    root,
    path: "hello.txt",
  });
  assert.equal(
    Buffer.from(savedFile.base64, "base64").toString(),
    "Edited over SSH from sushiAI\n",
  );
  await page.getByRole("button", { name: "Git changes", exact: true }).click();
  await page.locator('.file-row[title="hello.txt"]').click();
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll(".diff-add")).some((e) =>
      e.textContent.includes("+Edited over SSH from sushiAI"),
    ),
  );
  await page.screenshot({ path: "artifacts/remote-git.png" });
  await page.getByRole("button", { name: "Files", exact: true }).click();
  await page.locator('.file-row[title="art.svg"]').click();
  await page.waitForFunction(
    () => document.querySelector(".image-preview img")?.naturalWidth === 160,
  );
  await page.locator('.file-row[title="index.html"]').click();
  await page.getByTitle("Open file in browser").click();
  await page.waitForTimeout(1700);
  const guest = await desktop.evaluate(async ({ webContents }) => {
    const w = webContents
      .getAllWebContents()
      .find((w) => w.getType() === "webview");
    return w.executeJavaScript(
      '({text:document.body.innerText,color:getComputedStyle(document.querySelector("h1")).color,node:typeof process,bridge:typeof window.bridge})',
    );
  });
  assert.match(guest.text, /REMOTE_PREVIEW_OK/);
  assert.equal(guest.color, "rgb(255, 127, 80)");
  assert.equal(guest.bridge, "undefined");
  assert.equal(guest.node, "undefined");
  await page.keyboard.press("Escape");
  await page.getByTitle("Switch to tabs", { exact: true }).click();
  await page.getByRole("tab", { name: "zsh", exact: true }).click();
  await page.getByRole("tab", { name: "zsh", exact: true }).press("ArrowRight");
  assert.equal(
    await page
      .getByRole("tab", { name: "Files & Git", exact: true })
      .getAttribute("aria-selected"),
    "true",
  );
  for (const [width, height] of [
    [1440, 900],
    [1024, 700],
    [768, 600],
    [600, 480],
  ]) {
    await desktop.evaluate(
      ({ BrowserWindow }, { width, height }) =>
        BrowserWindow.getAllWindows()[0].setBounds({ width, height }),
      { width, height },
    );
    await page.waitForTimeout(300);
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      "No page overflow at " + width,
    );
    assert.equal(
      await page.locator(".workspace-canvas .panel:visible").count(),
      1,
    );
    await page.screenshot({ path: `artifacts/responsive-${width}.png` });
  }
  await desktop.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()[0].setBounds({ width: 1380, height: 900 }),
  );
  await page.getByRole("button", { name: "Sessions", exact: true }).click();
  await page
    .getByLabel("Session status", { exact: true })
    .selectOption("shells");
  await page.getByLabel("Select visible (1)", { exact: false }).check();
  await page
    .getByRole("button", { name: "End selected sessions", exact: true })
    .click();
  await page
    .getByRole("button", { name: "End 1 sessions", exact: true })
    .click();
  await page.waitForTimeout(800);
  const snapshot = (await request(socket, "session.snapshot")).snapshot;
  assert.ok(
    !snapshot.panes.some((x) => x.pane_id === pane),
    "Session manager really closes backend pane",
  );
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify(
      {
        passed: true,
        checks: [
          "remote terminal stream",
          "remote text/image",
          "text editor save over SSH",
          "Git diff",
          "HTML with relative CSS and isolated browser",
          "tabs keyboard navigation",
          "1440/1024/768/600 responsive layouts",
          "real remote session deletion",
        ],
      },
      null,
      2,
    ),
  );
} catch (e) {
  if (page)
    console.error(
      "UI failure evidence",
      await page.locator("body").innerText(),
    );
  if (page)
    await page
      .screenshot({ path: "artifacts/features-failure.png" })
      .catch(() => {});
  throw e;
} finally {
  await desktop?.close();
  if (workspace)
    await request(socket, "workspace.close", { workspace_id: workspace }).catch(
      () => {},
    );
  if (root?.startsWith("/tmp/sushiai-ui-fixture-"))
    await remote("import shutil;shutil.rmtree(" + JSON.stringify(root) + ")");
  await connections.close();
  await fs.rm(profile, { recursive: true, force: true });
}
