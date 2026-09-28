// One-command screenshot recipe for the Orchestrator panel's task detail
// view. Builds nothing itself: `npm run build` and `cargo build --release
// --manifest-path orchd/Cargo.toml` must already be done. Usage:
//
//   node .agents/skills/ui-evidence/scripts/orchestrator-panel.mjs <seed.json> <task title>
//
// <seed.json> is a JSON array of task objects (see SKILL.md for the shape);
// only `title` is required, everything else defaults. The tasks are written
// as task.json files into the throwaway profile's orchd data dir before the
// app starts; the app then spawns orchd on that profile as usual, which loads
// them. Only finished/waiting statuses are accepted, because orchd resumes
// queued/running/drafting tasks on start - a fixture must never run a harness.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { waitForExit } from "../../../../electron/orchestrator.cjs";

const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

const FIXTURE_STATUSES = new Set(["done", "failed", "stopped", "waiting"]);

/** A seed object completed to the task.json shape orchd's store loads. */
function taskJson(seed, repo, now) {
  const status = seed.status ?? "done";
  if (!FIXTURE_STATUSES.has(status))
    throw new Error(
      `seed task "${seed.title}": status ${status} would run on daemon start; use one of ${[...FIXTURE_STATUSES].join(", ")}`,
    );
  const id = randomUUID();
  const attempts = (seed.attempts ?? []).map((a, i) => ({
    n: i + 1,
    stage: "implement",
    routeId: "claude-sonnet",
    harness: "claude",
    model: "sonnet",
    reason: "tier default",
    resumed: false,
    startedAt: now,
    status: "passed",
    ...a,
  }));
  const costUsd =
    seed.costUsd ??
    attempts.reduce(
      (sum, a) => sum + (a.costUsd ?? 0) + (a.reviewCostUsd ?? 0),
      0,
    );
  return {
    goal: seed.title,
    criteria: [],
    verify: [],
    worktree: `${repo}-${id}`,
    branch: `task/${id}`,
    baseSha: "0".repeat(40),
    tier: "standard",
    decisions: [],
    archived: false,
    createdAt: now,
    updatedAt: now,
    ...seed,
    id,
    repo,
    status,
    attempts,
    costUsd,
  };
}

async function loadSeedTasks(seedPath) {
  const raw = JSON.parse(await fs.readFile(seedPath, "utf8"));
  if (!Array.isArray(raw))
    throw new Error("seed file must be a JSON array of tasks");
  return raw;
}

const [, , seedPath, title] = process.argv;
const report = { pageErrors: [] };

if (!seedPath || !title) {
  report.error = "usage: orchestrator-panel.mjs <seed.json> <task title>";
  console.log(JSON.stringify(report, null, 2));
  process.exitCode = 1;
} else {
  const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
  const dataDir = `${profile}/orchestrator`;
  let app = null;

  try {
    const tasks = await loadSeedTasks(seedPath);
    const now = Date.now();
    for (const seed of tasks) {
      const task = taskJson(seed, root, now);
      const dir = `${dataDir}/tasks/${task.id}`;
      await fs.mkdir(dir, { recursive: true });
      await fs.writeFile(`${dir}/task.json`, JSON.stringify(task, null, 2));
    }
    report.seededTitles = tasks.map((t) => t.title);

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

    // A fresh profile has no project open yet - a Local workspace pointed at
    // this repo gives the Orchestrator panel a `cwd` its seeded tasks' own
    // `repo` field matches (`task.list` filters on exact equality).
    await page.getByRole("button", { name: "New workspace" }).click();
    const workspaceDialog = page.getByRole("dialog", { name: "New workspace" });
    await workspaceDialog.locator('input[name="name"]').fill("Evidence");
    await workspaceDialog.getByLabel("Project folder").fill(root);
    await workspaceDialog
      .getByRole("button", { name: "Create workspace" })
      .click();

    await page.getByRole("button", { name: "Add panel" }).click();
    const panelDialog = page.getByRole("dialog", { name: "Add panel" });
    await panelDialog.getByRole("button", { name: "Orchestrator" }).click();
    // Maximized, not split beside the workspace's other panel: below ~640px
    // the panel's own container query hides the task list (orchestrator.css).
    await page.getByRole("button", { name: "Maximize Orchestrator" }).click();

    await page
      .locator(".orch-tasks, .empty-state")
      .first()
      .waitFor({ timeout: 15000 });
    const notBuilt = page.locator(".empty-state");
    if (await notBuilt.count()) {
      throw new Error(
        `the orchestrator panel could not open: ${(await notBuilt.innerText()).trim()}`,
      );
    }

    const row = page.locator(".orch-task-row").filter({
      has: page.locator(".orch-task-title", {
        hasText: new RegExp(`^${escapeRegExp(title)}$`),
      }),
    });
    // The list can still be one render behind the panel container just after
    // it mounts, so wait for the row itself rather than a one-shot count.
    try {
      await row.first().waitFor({ timeout: 10000 });
    } catch {
      throw new Error(
        `no seeded task titled "${title}" in the orchestrator panel`,
      );
    }
    await row.first().click();
    await page.locator(".orch-detail").waitFor();

    await page.screenshot({ path: shot("orchestrator-window") });
    await page
      .locator(".orch-detail")
      .screenshot({ path: shot("orchestrator-detail") });
    report.screenshots = {
      window: shot("orchestrator-window"),
      detail: shot("orchestrator-detail"),
    };
    report.selectedTitle = title;
  } catch (error) {
    report.error = String(error?.message ?? error);
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
}
