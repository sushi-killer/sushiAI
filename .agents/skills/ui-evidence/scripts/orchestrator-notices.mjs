// Screenshot recipe for orchd task notices on the desktop mascot: the
// done/failed/needs-input bubbles, the quick answer and the two "Open"
// landings. Builds nothing itself; run
//   npm run build && npm run build:orchd && node .agents/skills/ui-evidence/scripts/orchestrator-notices.mjs
// It seeds one landed done task with a fresh report, one failed task (last
// attempt failed with kind "verify") and one waiting task (options Delete it,
// Keep behind a flag, Stop) into a throwaway profile and opens the Evidence
// workspace with no Orchestrator panel. Each seeded task then goes through the
// real main-process path - `orchestratorNotice` -> the mascot queue - via the
// env-gated SUSHIAI_TEST_MASCOT seam in electron/main.cjs (never an IPC the
// renderer can reach). The mascot is its own window (found by its
// mascot.html URL). Saves artifacts/mascot-{done,failed,input,answered}.png,
// artifacts/orchestrator-open-{input,done}.png (reached by clicking Open on
// the mascot) and artifacts/mascot-report.json (bounds vs the primary work
// area, always-on-top, all-workspaces, the focused window before and after the
// mascot shows, visibility once the queue is empty). Prints the JSON report
// and exits non-zero on any failure.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { waitForExit } from "../../../../electron/orchestrator.cjs";

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
      options: ["Delete it", "Keep behind a flag", "Stop"],
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
      SUSHIAI_TEST_MASCOT: "1",
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

  // The real notice path: orchestratorNotice(task) -> attention -> mascot queue.
  const push = (title) =>
    app.evaluate((_, task) => globalThis.__sushiaiMascot.notify(task), {
      ...tasks[title],
    });
  const mainState = () =>
    app.evaluate(({ BrowserWindow }) => {
      const seam = globalThis.__sushiaiMascot;
      const mascot = seam.window();
      const focused = BrowserWindow.getFocusedWindow();
      return {
        focusedTitle: focused ? focused.getTitle() : null,
        focusedIsMascot: Boolean(mascot && focused && focused.id === mascot.id),
        mascot: mascot && {
          bounds: mascot.getBounds(),
          visible: mascot.isVisible(),
          isAlwaysOnTop: mascot.isAlwaysOnTop(),
          isVisibleOnAllWorkspaces: mascot.isVisibleOnAllWorkspaces(),
        },
        workArea: seam.workArea(),
        queue: seam.queue().map((item) => `${item.kind}:${item.title}`),
      };
    });
  const mascotPage = async () => {
    for (let i = 0; i < 100; i += 1) {
      const found = app.windows().find((w) => w.url().includes("mascot.html"));
      if (found) return found;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error("the mascot window never appeared");
  };
  let mascot = null;
  const showNotice = async (title, bubbleText) => {
    await push(title);
    mascot ??= await mascotPage();
    if (!mascot.__watched) {
      mascot.__watched = true;
      mascot.on("pageerror", (error) => report.pageErrors.push(error.message));
    }
    const bubble = mascot.locator(".bubble", { hasText: bubbleText }).first();
    await bubble.waitFor({ timeout: 10000 });
    // Let the hop settle so the frame is steady.
    await new Promise((resolve) => setTimeout(resolve, 700));
    return bubble;
  };
  const bubbleText = async () =>
    (await mascot.locator(".bubble").first().innerText())
      .replace(/\s+/g, " ")
      .trim();
  const dismiss = () =>
    mascot.getByRole("button", { name: "Dismiss" }).first().click();
  const shotMascot = (name) => mascot.screenshot({ path: shot(name) });

  report.focusBefore = await mainState();
  report.mascot = {};

  // Done.
  await showNotice("Landed feature", "Feature done: Landed feature");
  report.focusAfter = await mainState();
  await shotMascot("mascot-done");
  report.mascot.done = {
    text: await bubbleText(),
    sushi: await mascot.locator("img.sushi").count(),
    open: await mascot.getByRole("button", { name: "Open" }).count(),
    dismiss: await mascot.getByRole("button", { name: "Dismiss" }).count(),
  };
  await dismiss();

  // Needs input stays queued; the failed notice lands on top of it.
  await push("Needs a call");
  await showNotice("Verify broke", "Verify broke");
  await shotMascot("mascot-failed");
  report.mascot.failed = {
    text: await bubbleText(),
    queueCount: await mascot.locator(".queue span").innerText(),
    arrows: await mascot
      .getByRole("button", { name: /(Previous|Next) notice/ })
      .count(),
    open: await mascot.getByRole("button", { name: "Open" }).count(),
    dismiss: await mascot.getByRole("button", { name: "Dismiss" }).count(),
  };
  await dismiss();

  await mascot
    .locator(".bubble.input", { hasText: "Delete the legacy export path" })
    .waitFor();
  await shotMascot("mascot-input");
  report.mascot.input = {
    text: await bubbleText(),
    options: await mascot.locator(".bubble-options button").allInnerTexts(),
    answerField: await mascot.getByLabel("Answer", { exact: true }).count(),
    answerButton: await mascot.getByRole("button", { name: "Answer" }).count(),
    open: await mascot.getByRole("button", { name: "Open" }).count(),
    dismiss: await mascot.getByRole("button", { name: "Dismiss" }).count(),
  };

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
    await showNotice(
      title,
      title === "Landed feature" ? "Feature done" : title,
    );
    await mascot.getByRole("button", { name: "Open" }).first().click();
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

  // Quick answer: the Stop option, then the short confirmation.
  await mascot.locator(".bubble.input").waitFor();
  await mascot.getByRole("button", { name: "Stop", exact: true }).click();
  await mascot.locator(".bubble.answered").waitFor({ timeout: 10000 });
  await shotMascot("mascot-answered");
  report.mascot.answered = await bubbleText();
  await mascot
    .locator(".bubble")
    .waitFor({ state: "detached", timeout: 10000 });
  await page.waitForTimeout(500);
  report.afterEmpty = await mainState();
  report.screenshots = {
    mascotDone: shot("mascot-done"),
    mascotFailed: shot("mascot-failed"),
    mascotInput: shot("mascot-input"),
    mascotAnswered: shot("mascot-answered"),
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
  const box = report.focusAfter.mascot?.bounds;
  const area = report.focusAfter.workArea;
  if (
    !box ||
    box.x + box.width > area.x + area.width ||
    box.y + box.height > area.y + area.height ||
    area.x + area.width - (box.x + box.width) > 40 ||
    area.y + area.height - (box.y + box.height) > 40
  )
    problems.push("the mascot is not at the bottom-right of the work area");
  if (!report.focusAfter.mascot?.isAlwaysOnTop)
    problems.push("the mascot is not always on top");
  if (!report.focusAfter.mascot?.isVisibleOnAllWorkspaces)
    problems.push("the mascot is not visible on all workspaces");
  if (!report.focusAfter.mascot?.visible)
    problems.push("the mascot is not visible while a notice is queued");
  if (report.focusAfter.focusedIsMascot)
    problems.push("the mascot took focus when it showed");
  if (report.afterEmpty.mascot?.visible)
    problems.push("the mascot stayed visible with an empty queue");
  if (report.mascot.answered !== "Answered Thanks, the task carries on.")
    problems.push("no Answered confirmation");
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
  await fs
    .writeFile(
      `${root}/artifacts/mascot-report.json`,
      JSON.stringify(report, null, 2),
    )
    .catch(() => {});
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.pageErrors.length > 0) process.exitCode = 1;
}
