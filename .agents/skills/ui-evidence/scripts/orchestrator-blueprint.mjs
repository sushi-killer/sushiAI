// One-command screenshot recipe for the orchestrator pipeline blueprint: the
// diagram in a task's detail (a running task and a failed one) and the rail's
// Pipeline view with one stage's inspector open, each at a narrow (~520 px)
// and a wide (~1100 px) Orchestrator panel. Builds nothing itself:
// `npm run build` and `cargo build -p sushiai` must already be done. Usage:
//
//   node .agents/skills/ui-evidence/scripts/orchestrator-blueprint.mjs
//
// Never runs a real harness: `claude` and `codex` on the PATH given to the
// app are fake scripts that print an init line and sleep. See SKILL.md.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { stopDaemon } from "../../../../scripts/lib/daemon-binary.mjs";

const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;
const NARROW = 520;
const WIDE = 1100;

const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

const report = { screenshots: {}, widths: {}, pageErrors: [] };
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const daemonHome = `${profile}/sushiai`;
const dataDir = `${daemonHome}/orchestrator`;
const queuedId = randomUUID();
const failedId = randomUUID();
const queuedBranch = `task/${queuedId}`;
const queuedWorktree = `${profile}/wt`;
let worktreeAdded = false;
let branchAdded = false;
let app = null;

/** A fake harness: prints the first line harness.rs parses, then sleeps for
 * at most ten minutes so a leftover exits by itself. */
async function fakeHarness(name, line) {
  const file = `${profile}/bin/${name}`;
  await fs.writeFile(file, `#!/bin/sh\necho '${line}'\nexec sleep 600\n`);
  await fs.chmod(file, 0o755);
}

function taskJson(fields, now) {
  return {
    criteria: ["The blueprint shows the stage the task is in"],
    verify: [],
    tier: "standard",
    // What task.create copies from the default settings; a task with no
    // variant reads as all flags off and matches no preset.
    variant: { loopDetect: true, land: true },
    decisions: [],
    archived: false,
    createdAt: now,
    updatedAt: now,
    repo: root,
    ...fields,
  };
}

async function writeTask(task) {
  const dir = `${dataDir}/tasks/${task.id}`;
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(`${dir}/task.json`, JSON.stringify(task, null, 2));
}

try {
  const baseSha = git("rev-parse", "HEAD");
  const now = Date.now();

  await fs.mkdir(`${profile}/bin`, { recursive: true });
  await fakeHarness(
    "claude",
    '{"type":"system","subtype":"init","session_id":"fake","model":"sonnet"}',
  );
  await fakeHarness("codex", '{"type":"thread.started","thread_id":"fake"}');

  // (1) A failed task: its implement attempt failed the verify step.
  await writeTask(
    taskJson(
      {
        id: failedId,
        title: "Blueprint failed fixture",
        goal: "Blueprint failed fixture",
        status: "failed",
        worktree: `${profile}/wt-failed`,
        branch: `task/${failedId}`,
        baseSha,
        verify: ["npm test"],
        briefCheck: { done: true, feasibilityDone: true },
        costUsd: 0.84,
        createdAt: now - 60_000,
        updatedAt: now - 5_000,
        attempts: [
          {
            n: 1,
            stage: "implement",
            routeId: "claude-sonnet",
            harness: "claude",
            model: "sonnet",
            reason: "tier default",
            resumed: false,
            startedAt: now - 50_000,
            endedAt: now - 8_000,
            status: "failed",
            costUsd: 0.84,
            summary: "Added the parser; two tests still fail.",
            verify: [
              {
                command: "npm test",
                code: 1,
                tail: "2 failing",
                ms: 31_000,
              },
            ],
            failure: {
              kind: "verify",
              detail: "npm test exited 1: 2 failing",
              signature: "verify:npm test:2 failing",
            },
          },
        ],
      },
      now,
    ),
  );

  // (2) A queued task. The backlog entry stops the daemon resuming it on
  // start; the Start button does. Its worktree and branch are real, because
  // a start with a missing branch fails (git.rs restore_worktree).
  git("worktree", "add", "-b", queuedBranch, queuedWorktree, "HEAD");
  worktreeAdded = true;
  branchAdded = true;
  // The pipeline fixture does not need the repo's full dependency install.
  await fs.writeFile(
    `${queuedWorktree}/.sushiai/setup.json`,
    JSON.stringify({ setup: "true" }),
  );
  await writeTask(
    taskJson(
      {
        id: queuedId,
        title: "Blueprint queued fixture",
        goal: "Blueprint queued fixture",
        status: "queued",
        worktree: queuedWorktree,
        branch: queuedBranch,
        baseSha,
        verify: [],
        finalVerify: [],
        backlog: { bucket: "next", order: 0 },
        costUsd: 0,
        attempts: [],
      },
      now + 1,
    ),
  );

  // The daemon hosts the orchestrator module only when it is enabled.
  await fs.mkdir(`${daemonHome}/modules`, { recursive: true, mode: 0o700 });
  await fs.writeFile(`${daemonHome}/modules/orch.enabled`, "");
  await fs.mkdir(dataDir, { recursive: true });
  await fs.writeFile(
    `${dataDir}/settings.json`,
    JSON.stringify(
      {
        routes: [
          {
            id: "codex-luna",
            label: "Codex Luna",
            harness: "codex",
            model: "gpt-5.6-luna",
            strength: 1,
          },
          {
            id: "claude-sonnet",
            label: "Claude Sonnet",
            harness: "claude",
            model: "sonnet",
            strength: 2,
          },
          {
            id: "claude-opus",
            label: "Claude Opus",
            harness: "claude",
            model: "opus",
            strength: 3,
          },
        ],
        planner: "claude-opus",
        review: "claude-opus",
        tiers: {
          mechanical: "codex-luna",
          standard: "codex-luna",
          hard: "codex-luna",
        },
        sandbox: "native",
        allowedDomains: ["*"],
        protectedPaths: [],
        maxAttempts: 4,
        parallel: 2,
        presets: [
          {
            id: "blueprint-custom",
            label: "Blueprint custom",
            builtin: false,
            values: {
              judge: "claude-sonnet",
              tierRoutes: {
                mechanical: "codex-luna",
                standard: "codex-luna",
                hard: "codex-luna",
              },
            },
          },
        ],
        repoPresets: { [root]: "blueprint-custom" },
      },
      null,
      2,
    ),
  );

  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      BRIDGE_DATA_DIR: profile,
      // Never the owner's ~/.codex or ~/.sushiai/bin link.
      HOME: profile,
      CODEX_HOME: `${profile}/codex`,
      SUSHIAI_HOME: daemonHome,
      BRIDGE_DEV_URL: "",
      // orchd runs a bare `claude` / `codex` from PATH; these are the fakes.
      PATH: `${profile}/bin:${process.env.PATH}`,
    },
  });
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");

  await page.getByRole("button", { name: "Extensions", exact: true }).click();
  await page.getByRole("button", { name: "Enable Orchestrator" }).click();
  await page.getByRole("button", { name: "Disable Orchestrator" }).waitFor();
  await page.getByRole("button", { name: "Extensions", exact: true }).click();

  await page.getByRole("button", { name: "Add panel" }).first().click();
  await page
    .getByRole("dialog", { name: "Add panel" })
    .getByRole("button", { name: "Orchestrator" })
    .click();
  await page.getByRole("button", { name: "Maximize Orchestrator" }).click();
  await page
    .locator(".orch-rail, .orch-view-scroll.offline")
    .first()
    .waitFor({ timeout: 15000 });
  if (await page.locator(".orch-view-scroll.offline").count()) {
    throw new Error("the orchestrator panel could not open");
  }

  const panel = page.locator(".orchestrator-panel").first();
  const panelWidth = async () => (await panel.boundingBox())?.width ?? 0;
  /** Resize the hidden window until the panel measures about `target` px. */
  async function fit(target, label) {
    for (let i = 0; i < 4; i += 1) {
      const have = await panelWidth();
      if (Math.abs(have - target) <= 4) break;
      await app.evaluate(
        ({ BrowserWindow }, delta) => {
          const win = BrowserWindow.getAllWindows()[0];
          const [w, h] = win.getContentSize();
          win.setContentSize(Math.max(300, w + delta), Math.max(h, 800));
        },
        Math.round(target - have),
      );
      await page.waitForTimeout(400);
    }
    const width = Math.round(await panelWidth());
    report.widths[label] = width;
    return width;
  }

  const row = (title) =>
    page.locator(".ui-task-row").filter({
      has: page.locator(".ui-task-row-title", { hasText: title }),
    });
  const save = async (name, locator) => {
    await locator.screenshot({ path: shot(name) });
    report.screenshots[name] = shot(name);
  };
  const taskDetail = page.locator(".td");

  // Rail views and task rows exist only at the wide size (the panel's
  // container query hides the rail below 640 px), so every pick is made wide.
  await fit(WIDE, "wide");

  // Pipeline view with optional, implement-tier and judge inspectors open.
  await page
    .locator(".orch-rail")
    .getByRole("button", { name: /^Pipeline/ })
    .click();
  await page.locator(".bp").waitFor({ timeout: 10000 });
  // Several inspectors stay open together: an optional stage (enable
  // switch), Implement (three tier routes, max attempts), Judge (route) and
  // Checks (owner only).
  for (const stage of ["Advisor", "Implement", "Judge", "Checks"]) {
    await page
      .locator(".bp-flow .bp-node, .bp-branch .bp-node")
      .filter({
        has: page.locator(".bp-node-label").getByText(stage, { exact: true }),
      })
      .first()
      .click();
  }
  await page
    .locator(".bp-inspector")
    .getByText("Enabled", { exact: true })
    .waitFor();
  await page.getByLabel("Max attempts").waitFor();
  await page.getByLabel("Mechanical route").waitFor();
  await page.getByLabel("Standard route").waitFor();
  await page.getByLabel("Hard route").waitFor();
  await page.getByLabel("Judge route").waitFor();
  for (const [name, option] of [
    ["Mechanical route", "Codex Luna (gpt-5.6-luna)"],
    ["Judge route", "Claude Sonnet (sonnet)"],
    ["Judge route", "Auto: Claude Sonnet (sonnet)"],
  ]) {
    const found = await page
      .getByLabel(name)
      .evaluate(
        (select, text) =>
          [...select.options].some((item) => item.text === text),
        option,
      );
    if (!found) throw new Error(`${name} is missing option "${option}"`);
  }
  await page.getByText("shared checks: owner only").waitFor();
  const pipelineView = page.locator(".orch-view-scroll").first();
  await save("blueprint-repo-inspector-wide", pipelineView);
  await fit(NARROW, "narrow");
  await save("blueprint-repo-inspector-narrow", pipelineView);
  await fit(WIDE, "wide");

  // The failed task: Checks marked danger, the failed attempt's cost.
  await row("Blueprint failed fixture").first().click();
  await taskDetail.waitFor();
  await page.locator(".bp-node.failed").first().waitFor({ timeout: 10000 });
  await save("blueprint-failed-wide", taskDetail);
  await fit(NARROW, "narrow");
  await save("blueprint-failed-narrow", taskDetail);
  await fit(WIDE, "wide");

  // The queued task: Start, then wait for Implement to run. It sits in the backlog, which only the Plan view lists.
  await page
    .locator(".orch-rail")
    .getByRole("button", { name: /^Plan/ })
    .click();
  await page
    .locator(".plan-title", { hasText: "Blueprint queued fixture" })
    .first()
    .click();
  await taskDetail.waitFor();
  // The fake harness never answers a brief check, so the task takes the Fast
  // preset (brief check off) and goes straight to Implement.
  await taskDetail.getByLabel("Preset").selectOption({ label: "Fast" });
  await page
    .waitForFunction(
      async (id) => {
        const task = await window.bridge.orchestrator("task.get", { id });
        const pipeline = await window.bridge.orchestrator("pipeline.describe", {
          taskId: id,
        });
        return (
          task.variant?.maxAttempts === 2 &&
          task.variant?.briefCheckRoute === "" &&
          pipeline.presetId === "fast" &&
          pipeline.stages.find((stage) => stage.id === "implement")
            ?.maxAttempts === 2 &&
          pipeline.stages.find((stage) => stage.id === "brief-check")
            ?.enabled === false
        );
      },
      queuedId,
      { timeout: 10000, polling: 100 },
    )
    .catch(async () => {
      const alert = await page
        .getByRole("alert")
        .allInnerTexts()
        .catch(() => []);
      throw new Error(
        `the Fast preset was not applied: ${alert.join(" | ") || "no error shown"}`,
      );
    });
  await taskDetail.getByRole("button", { name: "Start", exact: true }).click();
  try {
    await page
      .locator(".bp-node.current", { hasText: "Implement" })
      .first()
      .waitFor({ timeout: 60000 });
  } catch (error) {
    const seen = await taskDetail.innerText().catch(() => "");
    throw new Error(
      `no running Implement stage: ${error.message}\n${seen.slice(0, 1500)}`,
    );
  }
  await taskDetail.getByText("attempt 1/2", { exact: false }).waitFor();
  await save("blueprint-running-wide", taskDetail);
  await fit(NARROW, "narrow");
  await save("blueprint-running-narrow", taskDetail);
  await fit(WIDE, "wide");

  // Stop it before the app closes, so no fake harness is left running.
  await taskDetail.getByRole("button", { name: "Stop task" }).click();
  await taskDetail.getByText("Stopped", { exact: true }).waitFor({
    timeout: 30000,
  });
  await save("blueprint-stopped-wide", taskDetail);
  await fit(NARROW, "narrow");
  await save("blueprint-stopped-narrow", taskDetail);
} catch (error) {
  report.error = String(error?.message ?? error);
} finally {
  if (app) await app.close().catch(() => {});
  stopDaemon(daemonHome);
  // Only this run's own worktree and branch; no other ref is touched.
  const cleanup = (args) => {
    try {
      git(...args);
    } catch (error) {
      report.cleanupError = String(error?.message ?? error);
    }
  };
  if (worktreeAdded) cleanup(["worktree", "remove", "--force", queuedWorktree]);
  if (branchAdded) cleanup(["branch", "-D", queuedBranch]);
  cleanup(["worktree", "prune"]);
  await fs.rm(profile, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 200,
  });
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.cleanupError || report.pageErrors.length > 0)
    process.exitCode = 1;
}
