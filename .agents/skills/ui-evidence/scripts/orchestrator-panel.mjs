// One-command screenshot recipe for the Orchestrator panel's task detail
// view. Builds nothing itself: `npm run build` and `cargo build --release
// --manifest-path orchd/Cargo.toml` must already be done. Usage:
//
//   node .agents/skills/ui-evidence/scripts/orchestrator-panel.mjs <seed.json> <task title>
//
// <seed.json> is a JSON array of task objects, or {tasks, proposals, notes} (see
// SKILL.md for the shape); only `title` is required on a task, everything
// else defaults. The tasks are written as task.json files into the throwaway
// profile's orchd data dir before the app starts; the app then spawns orchd
// on that profile as usual, which loads them. Each proposal is completed
// (id, repo, createdAt) and written to <dataDir>/evolution/proposals/<id>.json,
// the path orchd's store reads; when any were seeded the run also saves
// artifacts/orchestrator-proposals.png, a crop of the PROPOSALS list. Each note is completed (id, source
// "owner", createdAt) and written under this repo's root in
// <dataDir>/repo-notes.json ({repo: [note]}, what orchd's store reads); when
// any were seeded the run also saves artifacts/orchestrator-repo-notes.png,
// a crop of the REPO NOTES section. Only finished/waiting statuses are accepted, because orchd resumes
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

/** A seed object completed to the task.json shape orchd's store loads.
 * `ids` maps each seed's `key` to its generated id, so `parent`,
 * `dependsOn`, `followUps` and `followUpOf` can name other seeds by key. */
function taskJson(seed, repo, now, id, ids) {
  const status = seed.status ?? "done";
  if (!FIXTURE_STATUSES.has(status))
    throw new Error(
      `seed task "${seed.title}": status ${status} would run on daemon start; use one of ${[...FIXTURE_STATUSES].join(", ")}`,
    );
  const byKey = (key) => ids.get(key) ?? key;
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
  const {
    key: _key,
    parent,
    dependsOn,
    followUps,
    followUpOf,
    evidence: _evidence,
    ...rest
  } = seed;
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
    ...rest,
    ...(parent ? { parent: byKey(parent) } : {}),
    ...(dependsOn ? { dependsOn: dependsOn.map(byKey) } : {}),
    ...(followUps ? { followUps: followUps.map(byKey) } : {}),
    ...(followUpOf ? { followUpOf: byKey(followUpOf) } : {}),
    id,
    repo,
    status,
    attempts,
    costUsd,
  };
}

async function loadSeed(seedPath) {
  const raw = JSON.parse(await fs.readFile(seedPath, "utf8"));
  if (Array.isArray(raw)) return { tasks: raw, proposals: [], notes: [] };
  if (raw && Array.isArray(raw.tasks))
    return {
      tasks: raw.tasks,
      proposals: raw.proposals ?? [],
      notes: raw.notes ?? [],
    };
  throw new Error(
    "seed file must be a JSON array of tasks or {tasks, proposals, notes}",
  );
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
    const { tasks, proposals, notes } = await loadSeed(seedPath);
    const now = Date.now();
    const ids = new Map(
      tasks.filter((seed) => seed.key).map((seed) => [seed.key, randomUUID()]),
    );
    for (const [index, seed] of tasks.entries()) {
      const id = (seed.key && ids.get(seed.key)) || randomUUID();
      // Seed order is creation order, which is how subtasks are listed.
      const task = taskJson(seed, root, now + index, id, ids);
      const dir = `${dataDir}/tasks/${task.id}`;
      await fs.mkdir(dir, { recursive: true });
      // `evidence: {"<attempt n>": ["<image path>", ...]}` copies images into
      // runs/<n>/evidence/, where orchd keeps an attempt's screenshots, and
      // lists them on that attempt the way orchd records them.
      for (const [n, files] of Object.entries(seed.evidence ?? {})) {
        const to = `${dir}/runs/${n}/evidence`;
        await fs.mkdir(to, { recursive: true });
        const saved = [];
        for (const file of files) {
          const dest = `${to}/${file.split("/").pop()}`;
          await fs.copyFile(file, dest);
          saved.push(dest);
        }
        const attempt = task.attempts.find((a) => a.n === Number(n));
        if (attempt) attempt.evidence = saved;
      }
      await fs.writeFile(`${dir}/task.json`, JSON.stringify(task, null, 2));
    }
    if (!Array.isArray(proposals))
      throw new Error("proposals must be an array");
    const proposalsDir = `${dataDir}/evolution/proposals`;
    await fs.mkdir(proposalsDir, { recursive: true });
    for (const [index, seed] of proposals.entries()) {
      const id = seed.id ?? randomUUID();
      const proposal = { ...seed, id, repo: root, createdAt: now + index };
      await fs.writeFile(
        `${proposalsDir}/${id}.json`,
        JSON.stringify(proposal, null, 2),
      );
    }
    if (!Array.isArray(notes)) throw new Error("notes must be an array");
    if (notes.length > 0) {
      const seeded = notes.map((seed, index) => ({
        id: randomUUID(),
        source: "owner",
        ...seed,
        createdAt: now + index,
      }));
      await fs.mkdir(dataDir, { recursive: true });
      await fs.writeFile(
        `${dataDir}/repo-notes.json`,
        JSON.stringify({ [root]: seeded }, null, 2),
      );
    }
    report.seededTitles = tasks.map((t) => t.title);

    app = await electron.launch({
      args: ["."],
      cwd: root,
      env: {
        ...process.env,
        SUSHIAI_TEST_WINDOW: "hidden",
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
    // The orchestrator page (chat, plus the recurring failures above it) is
    // what the panel shows before a task is picked.
    await page
      .locator(".orch-main")
      .screenshot({ path: shot("orchestrator-home") });
    if (proposals.length > 0) {
      await page.locator(".orch-proposals").waitFor({ timeout: 10000 });
      await page
        .locator(".orch-proposals")
        .screenshot({ path: shot("orchestrator-proposals") });
    }
    if (notes.length > 0) {
      await page.locator(".orch-notes").waitFor({ timeout: 10000 });
      await page
        .locator(".orch-notes")
        .screenshot({ path: shot("orchestrator-repo-notes") });
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
      home: shot("orchestrator-home"),
      ...(proposals.length > 0
        ? { proposals: shot("orchestrator-proposals") }
        : {}),
      ...(notes.length > 0 ? { notes: shot("orchestrator-repo-notes") } : {}),
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
