import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { request } = require("../electron/herdr.cjs");
const { HERDR_CONTRACT } = require("../electron/herdr-contract.cjs");
const {
  startStreamDaemon,
  streamTestBinary,
} = require("./terminal-stream-fixture.cjs");
const root = process.cwd();
const daemon = await startStreamDaemon(streamTestBinary());
const otherDaemon = await startStreamDaemon(streamTestBinary());
const profile = await fs.mkdtemp("/tmp/sushiai-migration-evidence-");
await fs.mkdir(path.join(root, "artifacts"), { recursive: true });
const report = { pageErrors: [], screenshots: [] };
let app;
let workspaceId;
let otherWorkspaceId;
let concurrentWorkspaceId;
const readPid = async (file) => {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const pid = await fs.readFile(file, "utf8").catch(() => "");
    if (/^\d+$/.test(pid)) return pid;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error("The persistent shell did not answer the PID probe");
};
const workspaceKey = (id) =>
  `herdr:v2:${encodeURIComponent(daemon.socket)}:${encodeURIComponent(id)}`;
try {
  await promisify(execFile)("git", ["init", "--quiet", daemon.directory]);
  await promisify(execFile)("git", ["init", "--quiet", otherDaemon.directory]);
  const created = await request(daemon.socket, "workspace.create", {
    label: "Migration project",
    cwd: daemon.directory,
    focus: false,
  });
  workspaceId = created.workspace.workspace_id;
  const paneId = created.root_pane.pane_id;
  const otherCreated = await request(otherDaemon.socket, "workspace.create", {
    label: "Independent endpoint",
    cwd: otherDaemon.directory,
    focus: false,
  });
  otherWorkspaceId = otherCreated.workspace.workspace_id;
  assert.equal(otherWorkspaceId, workspaceId);
  assert.equal(otherCreated.root_pane.pane_id, paneId);
  const otherKey = (id) =>
    `herdr:v2:${encodeURIComponent(otherDaemon.socket)}:${encodeURIComponent(id)}`;
  await request(daemon.socket, "pane.rename", {
    pane_id: paneId,
    label: "Persistent shell",
  });
  const oldWorkspace = `herdr:local:${workspaceId}`;
  const oldPane = `herdr:local:${paneId}`;
  const layout = {
    type: "split",
    id: "saved-split",
    axis: "row",
    ratio: 0.62,
    a: { type: "leaf", id: oldPane },
    b: { type: "leaf", id: "local-files" },
  };
  const saved = {
    workspaces: [
      {
        id: oldWorkspace,
        herdrId: workspaceId,
        connection: daemon.socket,
        name: "Migration project",
        cwd: daemon.directory,
        panels: [
          {
            id: oldPane,
            herdrId: paneId,
            kind: "terminal",
            title: "Persistent shell",
          },
          { id: "local-files", kind: "files", title: "Local files" },
          {
            id: "conversation",
            kind: "chat",
            title: "Saved conversation",
            agent: "claude",
            messages: [
              {
                id: "message",
                role: "assistant",
                text: "Conversation survives migration.",
              },
            ],
          },
        ],
        layout,
      },
      {
        id: otherKey(otherWorkspaceId),
        herdrId: otherWorkspaceId,
        connection: otherDaemon.socket,
        name: "Independent endpoint",
        cwd: otherDaemon.directory,
        panels: [
          {
            id: otherKey(paneId),
            herdrId: paneId,
            kind: "terminal",
            title: "Independent shell",
          },
        ],
        layout: { type: "leaf", id: otherKey(paneId) },
      },
    ],
    activeId: oldWorkspace,
    socket: daemon.socket,
    routines: [],
    fontScale: 1,
    mode: "Code",
    selected: oldPane,
    zoomed: null,
    sidebar: true,
    views: { [oldWorkspace]: { tabMode: false, zoomed: null } },
    mergedLayouts: { [oldWorkspace]: layout },
    chatFocus: "conversation",
  };
  await fs.writeFile(
    path.join(profile, "workspace-state.json"),
    JSON.stringify(saved),
  );
  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      BRIDGE_DATA_DIR: profile,
      HERDR_SOCKET_PATH: daemon.socket,
      BRIDGE_DEV_URL: "",
      SHELL: "/bin/sh",
    },
  });
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.getByText("Persistent shell", { exact: true }).first().waitFor();
  await page.waitForFunction(() => {
    const state = JSON.parse(window.bridge.workspaceStateRead());
    return state.workspaces[0].id.startsWith("herdr:v2:");
  });
  report.restored = await page.evaluate(() =>
    JSON.parse(window.bridge.workspaceStateRead()),
  );
  assert.equal(report.restored.activeId, workspaceKey(workspaceId));
  assert.equal(report.restored.selected, workspaceKey(paneId));
  assert.equal(report.restored.workspaces[1].id, otherKey(otherWorkspaceId));
  assert.notEqual(
    report.restored.workspaces[0].id,
    report.restored.workspaces[1].id,
  );
  assert.notEqual(
    report.restored.workspaces[0].panels[0].id,
    report.restored.workspaces[1].panels[0].id,
  );
  report.sameInternalIdsIndependent = true;
  assert.equal(report.restored.workspaces[0].layout.ratio, 0.62);
  assert.equal(report.restored.workspaces[0].layout.a.id, workspaceKey(paneId));
  assert.equal(report.restored.workspaces[0].layout.b.id, "local-files");
  assert.equal(
    report.restored.workspaces[0].panels.find(
      (item) => item.id === "conversation",
    ).messages[0].text,
    "Conversation survives migration.",
  );
  await page.waitForFunction(
    () => !!document.querySelector(".panel-terminal.focused"),
  );
  await request(daemon.socket, "pane.send_input", {
    pane_id: paneId,
    text: "printf 'MIGRATION_TERMINAL_READY\\n'",
    keys: ["Enter"],
  });
  await page.waitForFunction(() =>
    document
      .querySelector(".panel-terminal .xterm-screen")
      ?.textContent.includes("MIGRATION_TERMINAL_READY"),
  );
  report.geometry = await page.locator(".panel").evaluateAll((panels) =>
    panels.map((panel) => {
      const bounds = panel.getBoundingClientRect();
      return {
        width: bounds.width,
        height: bounds.height,
        horizontalOverflow: panel.scrollWidth - panel.clientWidth,
        selected: panel.classList.contains("focused"),
      };
    }),
  );
  assert.ok(report.geometry.length >= 2);
  assert.equal(report.geometry.filter((panel) => panel.selected).length, 1);
  assert.ok(
    report.geometry.every((panel) => panel.width > 100 && panel.height > 100),
  );
  await page.screenshot({ path: "artifacts/herdr-migration-layout.png" });
  report.screenshots.push("artifacts/herdr-migration-layout.png");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("tab", { name: "Connections", exact: true }).click();
  await dialog
    .getByText(`Compatible · verified version ${HERDR_CONTRACT.version}`, {
      exact: true,
    })
    .waitFor();
  await dialog.screenshot({ path: "artifacts/herdr-compatibility.png" });
  report.screenshots.push("artifacts/herdr-compatibility.png");
  report.structuredError = await page.evaluate(async (endpoint) => {
    try {
      await window.bridge.herdr(endpoint, "pane.split", {
        target_pane_id: "w999999:p999999",
        direction: "right",
      });
    } catch (error) {
      return { code: error.code, message: error.message };
    }
  }, daemon.socket);
  assert.equal(report.structuredError.code, "pane_not_found");
  const checkout = path.join(profile, "launch-checkout");
  await fs.mkdir(checkout);
  await promisify(execFile)("git", ["init", "--quiet", checkout]);
  const requests = Array.from({ length: 20 }, (_, index) => ({
    operationId: `electron-intent-${index}`,
    endpoint: daemon.socket,
    cwd: checkout,
    label: "Concurrent IPC launches",
    kind: "terminal",
  }));
  const launched = await page.evaluate(
    (items) =>
      Promise.all(items.map((item) => window.bridge.sessionLaunch(item))),
    requests,
  );
  report.concurrentIpcErrors = launched
    .filter((item) => !item.ok)
    .map((item) => item.error);
  assert.ok(launched.every((item) => item.ok));
  assert.equal(new Set(launched.map((item) => item.value.workspaceId)).size, 1);
  assert.equal(new Set(launched.map((item) => item.value.paneId)).size, 20);
  concurrentWorkspaceId = launched[0].value.workspaceId;
  const repeated = await page.evaluate(
    (item) =>
      Promise.all(
        Array.from({ length: 20 }, () => window.bridge.sessionLaunch(item)),
      ),
    requests[0],
  );
  assert.ok(
    repeated.every(
      (item) => item.ok && item.value.paneId === launched[0].value.paneId,
    ),
  );
  const current = (await request(daemon.socket, "session.snapshot")).snapshot;
  assert.equal(
    current.panes.filter((item) => item.workspace_id === concurrentWorkspaceId)
      .length,
    20,
  );
  report.concurrentIpcLaunches = {
    distinct: 20,
    repeated: 20,
    workspaces: 1,
    panels: 20,
  };
  const beforeFile = path.join(profile, "before.pid");
  await request(daemon.socket, "pane.send_input", {
    pane_id: paneId,
    text: `printf '%s' "$$" > '${beforeFile}'`,
    keys: ["Enter"],
  });
  const beforePid = await readPid(beforeFile);
  await app.close();
  app = null;
  const afterFile = path.join(profile, "after.pid");
  await request(daemon.socket, "pane.send_input", {
    pane_id: paneId,
    text: `printf '%s' "$$" > '${afterFile}'`,
    keys: ["Enter"],
  });
  assert.equal(await readPid(afterFile), beforePid);
  report.processSurvivesAppClose = true;
  report.passed = true;
} finally {
  await app?.close();
  if (workspaceId)
    await request(daemon.socket, "workspace.close", {
      workspace_id: workspaceId,
    }).catch(() => {});
  if (concurrentWorkspaceId)
    await request(daemon.socket, "workspace.close", {
      workspace_id: concurrentWorkspaceId,
    }).catch(() => {});
  await daemon.close();
  if (otherWorkspaceId)
    await request(otherDaemon.socket, "workspace.close", {
      workspace_id: otherWorkspaceId,
    }).catch(() => {});
  await otherDaemon.close();
  await fs.rm(profile, { recursive: true, force: true });
  await fs.writeFile(
    "artifacts/herdr-ui-evidence.json",
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report, null, 2));
}
