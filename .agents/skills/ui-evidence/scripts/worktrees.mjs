import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { stopDaemon } from "../../../../scripts/lib/daemon-binary.mjs";

const root = process.cwd();
const temp = await fs.mkdtemp("/tmp/sushiai-worktree-evidence-");
const dataDir = path.join(temp, "profile");
const repo = path.join(temp, "checkout");
const worktree = path.join(temp, "feature-checkout");
const bin = path.join(temp, "bin");
const report = { pageErrors: [] };
let app;

function git(...args) {
  execFileSync("git", args, { stdio: "ignore" });
}

await fs.mkdir(repo, { recursive: true });
await fs.mkdir(bin, { recursive: true });
git("init", "--initial-branch=main", repo);
git("-C", repo, "config", "user.name", "Evidence User");
git("-C", repo, "config", "user.email", "evidence@example.test");
await fs.writeFile(path.join(repo, "README.md"), "worktree evidence\n");
git("-C", repo, "add", "README.md");
git("-C", repo, "commit", "-m", "initial");
git(
  "-C",
  repo,
  "remote",
  "add",
  "origin",
  "https://example.com/sushi/demo.git",
);
git("-C", repo, "worktree", "add", "-b", "feature/cleanup", worktree);
const fakeGh = path.join(bin, "gh");
await fs.writeFile(
  fakeGh,
  '#!/bin/sh\nprintf \'%s\\n\' \'[{"state":"MERGED","mergedAt":"2026-09-20T12:00:00Z"}]\'\n',
  { mode: 0o755 },
);
const mainPanel = {
  id: "main-agent",
  kind: "agent",
  title: "Main Agent",
  agent: "codex",
  started: false,
};
const featurePanel = {
  id: "feature-agent",
  kind: "agent",
  title: "Feature Agent",
  agent: "codex",
  started: false,
  sessionId: "feature-session",
  ended: true,
};
const workspaces = [
  {
    id: "main-checkout",
    name: "Sushi",
    cwd: repo,
    panels: [mainPanel],
    layout: { type: "leaf", id: mainPanel.id },
  },
  {
    id: "feature-checkout",
    name: "Sushi",
    cwd: worktree,
    localWorktree: true,
    panels: [featurePanel],
    layout: { type: "leaf", id: featurePanel.id },
  },
];
await fs.mkdir(dataDir, { recursive: true });
await fs.mkdir(path.join(root, "artifacts"), { recursive: true });
await fs.writeFile(
  path.join(dataDir, "workspace-state.json"),
  JSON.stringify({
    workspaces,
    activeId: "feature-checkout",
    selected: featurePanel.id,
    mode: "Code",
    sidebar: true,
    socket: missingSocket,
    workspaceGrouping: "grouped",
    routines: [],
    fontScale: 1,
  }),
);

try {
  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      BRIDGE_DATA_DIR: dataDir,
      BRIDGE_DEV_URL: "",
      SUSHIAI_HOME: path.join(dataDir, "sushiai"),
      SUSHIAI_TEST_WINDOW: "hidden",
    },
  });
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");
  await page.waitForFunction(() => {
    const row = [...document.querySelectorAll(".workspace-name")].find((item) =>
      item.innerText.includes("Sushi"),
    );
    return Boolean(row && document.querySelector(".workspace-panels"));
  });
  await page
    .locator(".workspace-panels button")
    .filter({ hasText: "Main Agent" })
    .waitFor();
  await page
    .locator(".workspace-panels button")
    .filter({ hasText: "Feature Agent" })
    .waitFor();
  const labels = async () =>
    page
      .locator(".workspace-name")
      .evaluateAll((items) =>
        items.map((item) => item.innerText.replace(/\s+/g, " ").trim()),
      );
  report.groupedRows = await labels();
  report.groupedPaneLabels = await page
    .locator(".workspace-panels button")
    .allTextContents();
  assert.equal(
    report.groupedRows.length,
    1,
    "grouped mode has one worktree row",
  );
  assert.equal(report.groupedPaneLabels.length, 2);
  const toggle = page.getByRole("button", { name: "Show as a flat list" });
  await toggle.click();
  await page.getByRole("button", { name: "Group by host" }).waitFor();
  await page
    .locator(".workspace-panels button")
    .filter({ hasText: "Feature Agent" })
    .waitFor();
  report.flatRows = await labels();
  report.flatPaneLabels = await page
    .locator(".workspace-panels button")
    .allTextContents();
  report.flatMeasurements = await page.evaluate(() => {
    const list = document.querySelector(".workspace-list");
    const names = [...document.querySelectorAll(".workspace-name")];
    return {
      rowCount: names.length,
      labelWidths: names.map((name) =>
        Math.round(name.getBoundingClientRect().width),
      ),
      overflow: list ? list.scrollWidth - list.clientWidth : null,
    };
  });
  report.flatStability = await page.evaluate(async () => {
    const counts = [];
    for (let index = 0; index < 40; index += 1) {
      counts.push(document.querySelectorAll(".workspace-name").length);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return { samples: counts.length, rowCounts: [...new Set(counts)] };
  });
  assert.equal(report.flatMeasurements.rowCount, 1);
  assert.ok(report.flatMeasurements.labelWidths.every((width) => width > 0));
  assert.equal(report.flatMeasurements.overflow, 0);
  assert.deepEqual(report.flatStability.rowCounts, [1]);
  await page.screenshot({
    path: path.join(root, "artifacts/worktrees-host-grouping-sidebar.png"),
  });
  await page.screenshot({
    path: path.join(root, "artifacts/worktrees-flat-sidebar.png"),
    clip: { x: 0, y: 0, width: 500, height: 880 },
  });

  await page
    .getByRole("button", { name: "Close Feature Agent", exact: true })
    .click();
  const dialog = page.locator(".modal");
  await dialog.getByText("Delete worktree feature/cleanup").waitFor();
  const checkbox = dialog.getByRole("checkbox", {
    name: "Delete worktree feature/cleanup",
  });
  await checkbox.waitFor();
  await page.waitForFunction(() => {
    const input = document.querySelector('.modal input[type="checkbox"]');
    return Boolean(input && !input.disabled && input.checked);
  });
  report.cleanupCheckedByDefault = await checkbox.isChecked();
  assert.equal(report.cleanupCheckedByDefault, true);
  report.cleanupButton = await dialog.locator(".danger").innerText();
  // The switch's knob slides for 120 ms after it turns on.
  await page.waitForTimeout(250);
  await page.screenshot({
    path: path.join(root, "artifacts/worktree-close-cleanup.png"),
  });
  await dialog.screenshot({
    path: path.join(root, "artifacts/worktree-close-cleanup-dialog.png"),
  });
  report.window = await app.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows()[0];
    const [width, height] = window.getContentSize();
    return {
      visible: window.isVisible(),
      focused: window.isFocused(),
      width,
      height,
    };
  });
  assert.equal(report.window.visible, false);
  assert.equal(report.window.focused, false);
} catch (error) {
  report.error = String(error?.message ?? error);
} finally {
  if (app) await app.close();
  stopDaemon(path.join(dataDir, "sushiai"));
  await fs.rm(temp, { recursive: true, force: true });
  console.log(JSON.stringify(report, null, 2));
}
if (report.error || report.pageErrors.length) process.exitCode = 1;
