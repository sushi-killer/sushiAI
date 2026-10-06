// Screenshot recipe for orchd task notices on the desktop mascot: the
// done/failed/needs-input bubbles, the quick answer and the two "Open"
// landings, Run again, Answer all in Inbox and the pill (Option-Space).
// Builds nothing itself; run
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
// mascot shows, visibility once the queue is empty). Also saves
// artifacts/mascot-{pill,presenting}.png (the pill reached through the seam's
// toggle, which is what Option-Space calls, and through a simulated fullscreen
// app), artifacts/mascot-stack.png and artifacts/mascot-inbox-open.png (the
// Inbox the pager's link opened). Run again restarts a separate failed task
// seeded in a throwaway git repo under the profile, with ORCHD_CLAUDE_BIN and
// ORCHD_CODEX_BIN pointed at /usr/bin/false so no agent really runs. Prints
// the JSON report and exits non-zero on any failure.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { execFileSync } from "node:child_process";
import { stopDaemon } from "../../../../scripts/lib/daemon-binary.mjs";

// "hidden" keeps the run off the owner's screen; "visible" is the opt-out for
// a run that must show the real mascot window (checks below adapt).
const windowMode = "hidden";
const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;

// Case (c): as long as a notice can get (body cap 2000, option cap 400), so it
// reaches the 70% cap even on a tall work area.
const LONG_TITLE =
  "orchd: a criterion checked by a command is never visual, and task.amend carries visual flags";
const CAP_SENTENCE =
  "The migration touches the queue, the planner and the review gate, and each of them keeps its own copy of the flag. ";
const CAP_BODY = CAP_SENTENCE.repeat(17).trim() + " Which way should it go?";
const CAP_WORDS =
  "keep the existing behaviour for running tasks, migrate stored tasks lazily when they are next opened, log every flag that changes, and leave the planner prompt alone until a follow-up task owns it".split(
    " ",
  );
const CAP_OPTIONS = ["Alpha", "Bravo", "Charlie", "Delta"].map(
  (name) => `${name}: ${[...CAP_WORDS, ...CAP_WORDS.slice(0, 20)].join(" ")}.`,
);

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
      askedBy: "verify",
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
  {
    title:
      "orchd: a criterion checked by a command is never visual, and task.amend carries visual flags",
    status: "waiting",
    costUsd: 0.2,
    question: {
      text: "The planner marked criterion 3 as visual, but its check is node --test tests/mascot.test.cjs, a command whose exit code decides it. A visual flag on it makes orchd demand a screenshot the implementer never needs to take, and the review stalls waiting for one. task.amend currently copies the visual flags from the old criteria by index, so reordering the criteria moves a flag onto the wrong one. How should a criterion that names a command be treated, and what should task.amend do with the flags?",
      options: [
        "Treat any criterion whose check names a verify command as non-visual, drop its visual flag at plan time, and have task.amend recompute every flag from the amended criteria instead of copying them from its old list by position.",
        "Keep the visual flag wherever the planner set it, but let a passing verify command satisfy the evidence requirement, and have task.amend carry flags over by matching criterion text rather than by index, so a reorder never moves one.",
        "Change nothing in the planner for now; only make task.amend clear every visual flag it cannot match to an identical criterion, and leave the command-versus-visual rule for a separate follow-up task that owns the planner prompt.",
      ],
      kind: "agent_question",
    },
    attempts: [
      {
        status: "blocked",
        summary: "Stopped to ask about the visual flag rule.",
        costUsd: 0.2,
        changedFiles: [],
        verify: [],
        gateBlocks: 0,
      },
    ],
  },
  {
    title: "Rerun me",
    status: "failed",
    costUsd: 0.12,
    rerun: true,
    attempts: [
      {
        status: "failed",
        summary: "The build broke.",
        costUsd: 0.12,
        changedFiles: [],
        verify: [],
        gateBlocks: 0,
        failure: {
          kind: "verify",
          detail: "npm run build exited 1",
          signature: "verify:npm run build",
        },
      },
    ],
  },
  {
    title: "Cap question",
    status: "waiting",
    costUsd: 0.2,
    question: {
      text: CAP_BODY,
      options: CAP_OPTIONS,
      kind: "agent_question",
    },
    attempts: [
      {
        status: "blocked",
        summary: "Stopped to ask a very long question.",
        costUsd: 0.2,
        changedFiles: [],
        verify: [],
        gateBlocks: 0,
      },
    ],
  },
];

const report = { pageErrors: [], everVisible: false, everFocused: false };
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const dataDir = `${profile}/orchestrator`;
let app = null;

try {
  const now = Date.now();
  const tasks = {};
  // Run again really starts a task: it gets its own repo, never this one.
  const rerunRepo = `${profile}/rerun-repo`;
  await fs.mkdir(rerunRepo, { recursive: true });
  const git = (...args) =>
    execFileSync("git", args, { cwd: rerunRepo, stdio: "ignore" });
  git("init", "-q", "-b", "main");
  await fs.writeFile(`${rerunRepo}/README.md`, "evidence\n");
  git("add", ".");
  git(
    "-c",
    "user.name=evidence",
    "-c",
    "user.email=evidence@example.invalid",
    "commit",
    "-qm",
    "init",
  );
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
    const { rerun, ...fields } = seed;
    const repo = rerun ? rerunRepo : root;
    const task = {
      goal: seed.title,
      criteria: [],
      verify: [],
      worktree: `${repo}-${id}`,
      branch: `task/${id}`,
      baseSha: "0".repeat(40),
      tier: "standard",
      decisions: [],
      archived: false,
      createdAt: now + index,
      updatedAt: now + index,
      ...fields,
      id,
      repo,
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
      SUSHIAI_TEST_WINDOW: windowMode,
      BRIDGE_DATA_DIR: profile,
      SUSHIAI_HOME: `${profile}/sushiai`,
      BRIDGE_DEV_URL: "",
      SUSHIAI_TEST_MASCOT: "1",
      ORCHD_CLAUDE_BIN: "/usr/bin/false",
      ORCHD_CODEX_BIN: "/usr/bin/false",
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
  const mainState = async () => {
    const state = await readState();
    if (state.mascot?.visible) report.everVisible = true;
    if (state.focusedIsMascot) report.everFocused = true;
    return state;
  };
  const readState = () =>
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
    sushi: await mascot.locator("svg.sushi").count(),
    open: await mascot.getByRole("button", { name: "View diff" }).count(),
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
    runAgain: await mascot.getByRole("button", { name: "Run again" }).count(),
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

  // Layout proof for the needs-input shots (a), (b) and (c).
  const layout = async (name, seed) => {
    const state = await readState();
    const inside = await mascot.evaluate(() => {
      const viewport = { w: window.innerWidth, h: window.innerHeight };
      const fits = (el) => {
        if (!el) return false;
        const r = el.getBoundingClientRect();
        return (
          r.top >= 0 &&
          r.left >= 0 &&
          r.bottom <= viewport.h &&
          r.right <= viewport.w
        );
      };
      const bubble = document.querySelector(".bubble.input");
      const title = bubble.querySelector("strong");
      const scroll = bubble.querySelector(".bubble-scroll");
      const style = getComputedStyle(title);
      const lineHeight =
        parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2;
      const buttons = [...bubble.querySelectorAll("button")];
      const byName = (label) =>
        buttons.find(
          (b) =>
            (b.getAttribute("aria-label") || b.textContent).trim() === label,
        );
      return {
        title: fits(title),
        answerField: fits(bubble.querySelector('input[aria-label="Answer"]')),
        open: fits(byName("Open task")),
        dismiss: fits(byName("Dismiss")),
        titleLines: Math.round(
          title.getBoundingClientRect().height / lineHeight,
        ),
        options: [...bubble.querySelectorAll(".bubble-options button")].map(
          (b) => ({
            innerText: b.innerText,
            accessibleName: b.getAttribute("aria-label") ?? b.textContent,
          }),
        ),
        scrollHeight: scroll.scrollHeight,
        clientHeight: scroll.clientHeight,
      };
    });
    const area = state.workArea;
    const box = state.mascot.bounds;
    const entry = {
      windowHeight: box.height,
      cap: Math.floor(0.7 * area.height),
      anchoredBottomRight:
        box.x + box.width === area.x + area.width - 12 &&
        box.y + box.height === area.y + area.height - 12,
      ...inside,
      scrolled: inside.scrollHeight > inside.clientHeight,
      seededOptions: seed.question.options,
    };
    report.layout ??= {};
    report.layout[name] = entry;
    return entry;
  };
  // Pushes a waiting task on top of the queue, shoots it and dismisses it.
  const layoutShot = async (title, file, name) => {
    await showNotice(title, title);
    await mascot.waitForTimeout(400);
    await shotMascot(file);
    await layout(name, tasks[title]);
    await dismiss();
    await mascot
      .locator(".bubble.input", { hasText: "Delete the legacy export path" })
      .waitFor();
  };
  await layout("short", tasks["Needs a call"]);
  await layoutShot(LONG_TITLE, "mascot-input-long", "long");
  await layoutShot("Cap question", "mascot-input-cap", "cap");

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
      .locator(".ui-task-row.selected .ui-task-row-title")
      .first()
      .innerText()
      .catch(() => "");

  async function openFrom(title, name, selector) {
    await showNotice(
      title,
      title === "Landed feature" ? "Feature done" : title,
    );
    await mascot
      .getByRole("button", { name: /^(Open|View diff)/ })
      .first()
      .click();
    await page.locator(".td").waitFor({ timeout: 15000 });
    await page.waitForFunction(
      ({ sel, expected }) => {
        const picked = document.querySelector(
          ".ui-task-row.selected .ui-task-row-title",
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

  // The panel-side steps depend on the task view's DOM; a break there is
  // recorded and the mascot checks below still run.
  try {
    report.openInput = await openFrom("Needs a call", "input", ".td-question");
    // Back to an unzoomed canvas before the second landing.
    await page.keyboard.press("Escape");
    // Start the second landing from a section page: opening must leave it.
    await page.getByRole("button", { name: /^Routines/ }).click();
    const routines = page.getByRole("heading", { name: "Routines" });
    await routines.first().waitFor({ timeout: 10000 });
    report.openedFrom = "Routines section page";
    report.openDone = await openFrom("Landed feature", "done", ".td-report");
    report.leftSection = (await routines.count()) === 0;
  } catch (error) {
    report.openFlowError = String(error.message ?? error).split("\n")[0];
  }

  // Quick answer: the Stop option, then the short confirmation.
  await mascot.locator(".bubble.input").waitFor();
  await mascot.getByRole("button", { name: "Stop", exact: true }).click();
  await mascot.getByRole("button", { name: "Send answer" }).click();
  await mascot.locator(".bubble.answered").waitFor({ timeout: 10000 });
  await shotMascot("mascot-answered");
  report.mascot.answered = await bubbleText();
  await mascot
    .locator(".bubble")
    .waitFor({ state: "detached", timeout: 10000 });
  await page.waitForTimeout(500);
  report.afterEmpty = await mainState();

  // The pill: Option-Space calls the seam's toggle; so does a fullscreen app.
  const seam = (name, ...args) =>
    app.evaluate(
      (_, [call, rest]) => globalThis.__sushiaiMascot[call](...rest),
      [name, args],
    );
  // Two still-waiting questions: they never expire, so the pager stays.
  await showNotice("Cap question", "Cap question");
  await showNotice(LONG_TITLE, LONG_TITLE);
  await seam("toggle");
  await mascot.locator(".pill").waitFor({ timeout: 5000 });
  await mascot.waitForTimeout(400);
  await shotMascot("mascot-pill");
  report.pill = {
    text: (await mascot.locator(".pill").innerText()).replace(/\s+/g, " "),
    kbd: await mascot.locator(".pill .kbd").innerText(),
  };
  await seam("toggle");
  await mascot.locator(".queue").waitFor({ timeout: 5000 });
  report.pill.expandedAgain = (await mascot.locator(".pill").count()) === 0;

  await seam("setPresenting", true);
  await mascot.locator(".pill").waitFor({ timeout: 5000 });
  await push("Verify broke");
  await mascot.waitForTimeout(700);
  report.presenting = {
    pillWhilePresenting: (await mascot.locator(".pill").count()) === 1,
    queue: (await readState()).queue.length,
  };
  await shotMascot("mascot-presenting");
  await seam("setPresenting", false);
  await mascot.locator(".queue").waitFor({ timeout: 5000 });
  report.presenting.expandedAfter =
    (await mascot.locator(".pill").count()) === 0;
  await dismiss(); // the failed notice on top; two stay for the pager
  await mascot.locator(".queue").waitFor({ timeout: 5000 });
  await mascot.waitForTimeout(400);
  await shotMascot("mascot-stack");

  // Answer all in Inbox hands the queue to the main window's Inbox.
  await mascot.getByRole("button", { name: "Answer all in Inbox" }).click();
  const inboxHeading = page.getByRole("heading", { name: "Inbox", level: 1 });
  await inboxHeading.waitFor({ timeout: 10000 });
  await page.waitForTimeout(400);
  await page.screenshot({ path: shot("mascot-inbox-open") });
  report.inbox = { opened: (await inboxHeading.count()) === 1 };
  // A second click keeps the Inbox open rather than toggling it shut.
  await mascot.getByRole("button", { name: "Answer all in Inbox" }).click();
  await page.waitForTimeout(500);
  report.inbox.stillOpen = (await inboxHeading.count()) === 1;

  // Run again restarts the task through orchd's task.start.
  const rerunTask = tasks["Rerun me"];
  const rerunFile = `${dataDir}/tasks/${rerunTask.id}/task.json`;
  await showNotice("Rerun me", "Rerun me");
  await mascot.getByRole("button", { name: "Run again" }).first().click();
  await mascot
    .locator(".bubble", { hasText: "Rerun me" })
    .waitFor({ state: "detached", timeout: 10000 });
  let restarted = null;
  for (let i = 0; i < 50 && !restarted; i += 1) {
    const saved = JSON.parse(await fs.readFile(rerunFile, "utf8"));
    if (
      saved.updatedAt !== rerunTask.updatedAt ||
      saved.status !== "failed" ||
      saved.attempts.length !== rerunTask.attempts.length
    )
      restarted = {
        status: saved.status,
        attempts: saved.attempts.length,
        updatedAtChanged: saved.updatedAt !== rerunTask.updatedAt,
      };
    else await new Promise((resolve) => setTimeout(resolve, 200));
  }
  report.rerun = { noticeGone: true, restarted };
  report.screenshots = {
    mascotDone: shot("mascot-done"),
    mascotFailed: shot("mascot-failed"),
    mascotInput: shot("mascot-input"),
    mascotAnswered: shot("mascot-answered"),
    mascotPill: shot("mascot-pill"),
    mascotPresenting: shot("mascot-presenting"),
    mascotStack: shot("mascot-stack"),
    inboxOpen: shot("mascot-inbox-open"),
    openInput: shot("orchestrator-open-input"),
    openDone: shot("orchestrator-open-done"),
  };

  const problems = [];
  if (!report.openFlowError) {
    if (report.panelBefore !== 0)
      problems.push("an Orchestrator panel existed before opening");
    if (report.openInput.selectedTitle !== "Needs a call")
      problems.push("input open selected the wrong task");
    if (!report.openInput.inViewport)
      problems.push(".td-question is not in view");
    if (report.openDone.selectedTitle !== "Landed feature")
      problems.push("done open selected the wrong task");
    if (!report.leftSection) problems.push("the section page was not left");
    if (!report.openDone.inViewport) problems.push(".td-report is not in view");
  }
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
  if (windowMode === "visible" && !report.focusAfter.mascot?.visible)
    problems.push("the mascot is not visible while a notice is queued");
  if (report.focusAfter.focusedIsMascot)
    problems.push("the mascot took focus when it showed");
  if (windowMode === "visible" && report.afterEmpty.mascot?.visible)
    problems.push("the mascot stayed visible with an empty queue");
  if (windowMode === "hidden") {
    if (report.everVisible)
      problems.push("the mascot was visible in hidden mode");
    if (report.everFocused)
      problems.push("the mascot took focus in hidden mode");
  }
  for (const [name, entry] of Object.entries(report.layout ?? {})) {
    if (!(entry.title && entry.answerField && entry.open && entry.dismiss))
      problems.push(
        `${name}: title, Answer, Open or Dismiss is not fully inside`,
      );
    if (!entry.anchoredBottomRight)
      problems.push(`${name}: the mascot is not anchored bottom-right`);
    if (entry.windowHeight > entry.cap)
      problems.push(`${name}: the window is taller than the cap`);
    if (name === "cap") {
      if (!entry.scrolled)
        problems.push("cap: the scroll area is not scrolled");
      if (entry.windowHeight !== entry.cap)
        problems.push("cap: the window height is not the cap");
    } else if (entry.scrolled)
      problems.push(`${name}: the scroll area scrolled`);
    if (name === "long" && entry.titleLines !== 2)
      problems.push(`long: the title has ${entry.titleLines} lines, not 2`);
    const seeded = entry.seededOptions;
    const ok =
      entry.options.length === seeded.length &&
      entry.options.every(
        (o, i) => o.innerText === seeded[i] && o.accessibleName === seeded[i],
      );
    if (!ok) problems.push(`${name}: option text or accessible name differs`);
  }
  if (!report.mascot.failed.runAgain)
    problems.push("the failed bubble has no Run again");
  if (report.pill.kbd !== "\u2325 Space")
    problems.push("the pill does not show the Option-Space key");
  if (!report.pill.expandedAgain) problems.push("toggle did not expand again");
  if (!report.presenting.pillWhilePresenting)
    problems.push("a notice expanded the mascot while presenting");
  if (!report.presenting.expandedAfter)
    problems.push("the mascot stayed a pill after presenting ended");
  if (!report.inbox.opened) problems.push("Answer all did not open the Inbox");
  if (!report.inbox.stillOpen)
    problems.push("a second Answer all closed the Inbox");
  if (!report.rerun.restarted)
    problems.push("Run again did not restart the task");
  if (!/the task carries on\.$/.test(report.mascot.answered))
    problems.push("no Answered confirmation");
  if (report.openFlowError) problems.push(`open flow: ${report.openFlowError}`);
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
  stopDaemon(`${profile}/sushiai`);
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
