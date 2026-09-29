// Screenshot recipe for orchd task notices: the three toasts and the two
// "Open" landings. Builds nothing itself; run
//   npm run build && npm run build:orchd && node .agents/skills/ui-evidence/scripts/orchestrator-notices.mjs
// It seeds one landed done task with a fresh report, one failed task (last attempt failed with
// kind "verify") and one waiting task into a throwaway profile, opens the
// Evidence workspace with no Orchestrator panel, then emits each notice as
// the `orchestrator-notice` IPC (built by the same `orchestratorNotice` main
// uses). Saves artifacts/orchestrator-toast-{done,failed,input}.png and, after
// clicking Open on the input and done toasts,
// artifacts/orchestrator-open-{input,done}.png (the done landing scrolls the
// task's Report section, `.orch-report`, into view). Prints a JSON report and
// exits non-zero on any failure.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { randomUUID } from "node:crypto";
import {
  orchestratorNotice,
  waitForExit,
} from "../../../../electron/orchestrator.cjs";

const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;

const SEEDS = [
  {
    title: "Landed feature",
    status: "done",
    landedSha: "0123456789abcdef0123456789abcdef01234567",
    baseRef: "main",
    costUsd: 1.42,
    report:
      "# Landed feature\n\n**Outcome:** done and landed on main.\n\n- Added the feature and its tests\n- verify passed\n",
    reportAt: Date.now(),
    attempts: [
      {
        status: "passed",
        summary: "Added the feature and its tests; verify passed.",
        costUsd: 1.42,
        changedFiles: ["src/feature.ts", "tests/feature.test.cjs"],
        verify: [],
        gateBlocks: 0,
      },
    ],
  },
  {
    title: "Verify broke",
    status: "failed",
    costUsd: 0.87,
    attempts: [
      {
        status: "failed",
        summary: "Changed the parser; the unit tests still fail.",
        costUsd: 0.87,
        changedFiles: ["src/parser.ts"],
        verify: [],
        gateBlocks: 0,
        failure: {
          kind: "verify",
          detail: "npm test exited 1",
          signature: "verify:npm test",
        },
      },
    ],
  },
  {
    title: "Needs a call",
    status: "waiting",
    costUsd: 0.31,
    question: {
      text: "Delete the legacy export path, or keep it behind a flag?",
      options: ["Delete it", "Keep behind a flag"],
      kind: "agent_question",
    },
    attempts: [
      {
        status: "blocked",
        summary: "Stopped to ask before removing a public export.",
        costUsd: 0.31,
        changedFiles: [],
        verify: [],
        gateBlocks: 0,
      },
    ],
  },
];

const report = { pageErrors: [] };
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const dataDir = `${profile}/orchestrator`;
let app = null;

try {
  const now = Date.now();
  const tasks = {};
  for (const [index, seed] of SEEDS.entries()) {
    const id = randomUUID();
    const attempts = seed.attempts.map((a, i) => ({
      n: i + 1,
      stage: "implement",
      routeId: "claude-sonnet",
      harness: "claude",
      model: "sonnet",
      reason: "tier default",
      resumed: false,
      startedAt: now,
      ...a,
    }));
    const task = {
      goal: seed.title,
      criteria: [],
      verify: [],
      worktree: `${root}-${id}`,
      branch: `task/${id}`,
      baseSha: "0".repeat(40),
      tier: "standard",
      decisions: [],
      archived: false,
      createdAt: now + index,
      updatedAt: now + index,
      ...seed,
      id,
      repo: root,
      attempts,
    };
    tasks[seed.title] = task;
    const dir = `${dataDir}/tasks/${id}`;
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(`${dir}/task.json`, JSON.stringify(task, null, 2));
  }
  await fs.mkdir(`${root}/artifacts`, { recursive: true });

  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      BRIDGE_DATA_DIR: profile,
      HERDR_SOCKET_PATH: `${profile}/no-herdr.sock`,
      BRIDGE_DEV_URL: "",
    },
  });
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");

  // A Local workspace on this repo: the notices' `repo` matches its cwd. No
  // Orchestrator panel is added - opening a notice has to add it.
  await page.getByRole("button", { name: "New workspace" }).click();
  const workspaceDialog = page.getByRole("dialog", { name: "New workspace" });
  await workspaceDialog.locator('input[name="name"]').fill("Evidence");
  await workspaceDialog.getByLabel("Project folder").fill(root);
  await workspaceDialog
    .getByRole("button", { name: "Create workspace" })
    .click();
  await page.waitForSelector(".panel-agent, .panel-terminal");
  report.panelBefore = await page.locator(".orchestrator-panel").count();

  const emit = (notice) =>
    app.evaluate(({ BrowserWindow }, value) => {
      BrowserWindow.getAllWindows()[0].webContents.send(
        "orchestrator-notice",
        value,
      );
    }, notice);
  const noticeFor = (title) => orchestratorNotice(tasks[title]);
  const dismissAll = async () => {
    for (const button of await page
      .getByRole("button", { name: "Dismiss" })
      .all())
      await button.click();
  };
  // A crop of the window's bottom-right corner, wide enough to read.
  const corner = async () => {
    const { width, height } = await page.evaluate(() => ({
      width: window.innerWidth,
      height: window.innerHeight,
    }));
    return {
      x: Math.max(0, width - 560),
      y: Math.max(0, height - 260),
      width: Math.min(560, width),
      height: Math.min(260, height),
    };
  };

  report.toasts = {};
  for (const [name, title] of [
    ["done", "Landed feature"],
    ["failed", "Verify broke"],
    ["input", "Needs a call"],
  ]) {
    await emit(noticeFor(title));
    const toast = page.locator(".orch-toast").first();
    await toast.waitFor();
    await page.screenshot({
      path: shot(`orchestrator-toast-${name}`),
      clip: await corner(),
    });
    report.toasts[name] = {
      text: (await toast.innerText()).replace(/\s+/g, " ").trim(),
      mascot: await toast.locator("img.orch-toast-mascot").count(),
      role: await toast.getAttribute("role"),
      box: await toast.boundingBox(),
    };
    await dismissAll();
  }

  const measure = (selector) =>
    page.evaluate((sel) => {
      const el = document.querySelector(sel);
      if (!el) return { present: false, inViewport: false };
      const r = el.getBoundingClientRect();
      const inViewport =
        r.bottom > 0 &&
        r.right > 0 &&
        r.top < window.innerHeight &&
        r.left < window.innerWidth;
      return { present: true, inViewport };
    }, selector);
  const selectedTitle = () =>
    page
      .locator(".orch-task-row.selected .orch-task-title")
      .first()
      .innerText()
      .catch(() => "");

  async function openFrom(title, name, selector) {
    await emit(noticeFor(title));
    await page.locator(".orch-toast").first().waitFor();
    await page
      .locator(".orch-toast")
      .first()
      .getByRole("button", { name: "Open" })
      .click();
    await page.locator(".orch-detail").waitFor({ timeout: 15000 });
    await page.waitForFunction(
      ({ sel, expected }) => {
        const picked = document.querySelector(
          ".orch-task-row.selected .orch-task-title",
        );
        if (picked?.textContent !== expected) return false;
        const el = document.querySelector(sel);
        if (!el) return false;
        const r = el.getBoundingClientRect();
        return r.bottom > 0 && r.top < window.innerHeight;
      },
      { sel: selector, expected: title },
      { timeout: 10000 },
    );
    // The panel fills the canvas: exactly one pane is left to maximize.
    await page.waitForFunction(
      () =>
        document.querySelectorAll('button[aria-label^="Maximize"]').length ===
        1,
      undefined,
      { timeout: 10000 },
    );
    await page.screenshot({ path: shot(`orchestrator-open-${name}`) });
    return {
      selectedTitle: await selectedTitle(),
      target: selector,
      ...(await measure(selector)),
      panelCount: await page.locator(".orchestrator-panel").count(),
    };
  }

  report.openInput = await openFrom("Needs a call", "input", ".orch-question");
  // Back to an unzoomed canvas before the second landing.
  await page.keyboard.press("Escape");
  // Start the second landing from a section page: opening must leave it.
  await page.getByRole("button", { name: /^Routines/ }).click();
  const routines = page.getByRole("heading", { name: "Routines" });
  await routines.first().waitFor({ timeout: 10000 });
  report.openedFrom = "Routines section page";
  report.openDone = await openFrom("Landed feature", "done", ".orch-report");
  report.leftSection = (await routines.count()) === 0;
  report.screenshots = {
    toastDone: shot("orchestrator-toast-done"),
    toastFailed: shot("orchestrator-toast-failed"),
    toastInput: shot("orchestrator-toast-input"),
    openInput: shot("orchestrator-open-input"),
    openDone: shot("orchestrator-open-done"),
  };

  const problems = [];
  if (report.panelBefore !== 0)
    problems.push("an Orchestrator panel existed before opening");
  if (report.openInput.selectedTitle !== "Needs a call")
    problems.push("input open selected the wrong task");
  if (!report.openInput.inViewport)
    problems.push(".orch-question is not in view");
  if (report.openDone.selectedTitle !== "Landed feature")
    problems.push("done open selected the wrong task");
  if (!report.leftSection) problems.push("the section page was not left");
  if (!report.openDone.inViewport) problems.push(".orch-report is not in view");
  if (problems.length) report.error = problems.join("; ");
} catch (error) {
  report.error = String(error?.message ?? error);
  // Keep the last frame: a failed run is diagnosed from what was on screen.
  const page = await app?.firstWindow().catch(() => null);
  await page
    ?.screenshot({ path: shot("orchestrator-notices-failure") })
    .catch(() => {});
} finally {
  if (app) await app.close().catch(() => {});
  // orchd outlives the app on purpose; this throwaway profile's daemon must not.
  const orchdPid = await fs
    .readFile(`${dataDir}/orchd.pid`, "utf8")
    .catch(() => "");
  if (Number(orchdPid)) {
    try {
      process.kill(Number(orchdPid), "SIGTERM");
    } catch {
      // already gone
    }
    await waitForExit(Number(orchdPid));
  }
  await fs.rm(profile, { recursive: true, force: true });
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.pageErrors.length > 0) process.exitCode = 1;
}
