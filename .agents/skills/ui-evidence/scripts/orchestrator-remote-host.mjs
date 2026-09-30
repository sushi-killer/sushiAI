// Screenshot recipe for the Orchestrator panel's host selector and a remote
// task, with no real SSH host: a fake ssh (tests/fixtures/fake-ssh.cjs) runs
// the "remote" commands locally under a throwaway HOME, so the app really
// uploads the local orchd binary, starts it detached there and talks to it
// through a forwarded socket. `npm run build` and
// `cargo build --release --manifest-path orchd/Cargo.toml` must be done.
//
//   node .agents/skills/ui-evidence/scripts/orchestrator-remote-host.mjs
//
// Saves artifacts/orchestrator-host-selector.png (the panel on the remote
// host) and artifacts/orchestrator-host-local.png (the same panel on Local),
// prints a JSON report and exits non-zero with `error` set on any failure.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";

const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;
const TOOLS =
  "cat mkdir tar mv cp chmod rm uname kill nohup sleep dirname setsid git".split(
    " ",
  );
const report = { pageErrors: [] };
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
let app = null;

try {
  await fs.mkdir(`${root}/artifacts`, { recursive: true });
  const home = `${profile}/remote-home`;
  const bin = `${profile}/remote-bin`;
  const repo = `${home}/work/api`;
  await fs.mkdir(repo, { recursive: true });
  await fs.mkdir(bin);
  for (const name of TOOLS) {
    try {
      const real = execFileSync("/bin/sh", ["-c", `command -v ${name}`], {
        encoding: "utf8",
      }).trim();
      if (real) await fs.symlink(real, `${bin}/${name}`);
    } catch {
      // A tool this machine lacks (setsid on macOS): the app falls back.
    }
  }
  // One waiting task already on the remote daemon's disk.
  const id = randomUUID();
  const now = Date.now();
  const dir = `${home}/.sushiai/orchestrator/tasks/${id}`;
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(
    `${dir}/task.json`,
    JSON.stringify({
      id,
      title: "Add rate limiting to the API",
      goal: "Add rate limiting to the API",
      request: "Add rate limiting to the API",
      criteria: [],
      verify: [],
      repo,
      worktree: `${repo}-${id}`,
      branch: `task/${id}`,
      baseSha: "0".repeat(40),
      tier: "standard",
      status: "waiting",
      question: { text: "Per IP or per API key?", options: ["IP", "Key"] },
      decisions: [],
      attempts: [],
      archived: false,
      costUsd: 0,
      createdAt: now,
      updatedAt: now,
    }),
  );
  const log = `${profile}/ssh.log`;
  await fs.writeFile(log, "");
  await fs.writeFile(`${profile}/ssh.json`, JSON.stringify({ home, bin, log }));
  const ssh = `${profile}/ssh`;
  await fs.writeFile(
    ssh,
    `#!${process.execPath}\nrequire(${JSON.stringify(`${root}/tests/fixtures/fake-ssh.cjs`)});\n`,
    { mode: 0o755 },
  );
  // The owner's Connections profile, already enabled for the Orchestrator.
  const profileId = randomUUID();
  await fs.mkdir(`${profile}/userData`, { recursive: true });
  await fs.writeFile(
    `${profile}/connections.json`,
    JSON.stringify([
      {
        id: profileId,
        name: "Build box",
        host: "build-box",
        socket: "~/.herdr.sock",
      },
    ]),
  );
  await fs.writeFile(
    `${profile}/orchestrator-hosts.json`,
    JSON.stringify([profileId]),
  );

  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      SUSHIAI_TEST_SSH: ssh,
      FAKE_SSH_CONFIG: `${profile}/ssh.json`,
      BRIDGE_DATA_DIR: profile,
      HERDR_SOCKET_PATH: `${profile}/no-herdr.sock`,
      BRIDGE_DEV_URL: "",
    },
  });
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");
  await page.getByRole("button", { name: "New workspace" }).click();
  const workspaceDialog = page.getByRole("dialog", { name: "New workspace" });
  await workspaceDialog.locator('input[name="name"]').fill("Evidence");
  await workspaceDialog.getByLabel("Project folder").fill(root);
  await workspaceDialog
    .getByRole("button", { name: "Create workspace" })
    .click();
  await page.getByRole("button", { name: "Add panel" }).click();
  await page
    .getByRole("dialog", { name: "Add panel" })
    .getByRole("button", { name: "Orchestrator" })
    .click();
  await page.getByRole("button", { name: "Maximize Orchestrator" }).click();

  const toggle = page.locator(".orch-rail .orch-host-toggle");
  await toggle.waitFor({ timeout: 15000 });
  await page.locator(".orchestrator-panel").first().waitFor({ timeout: 15000 });
  await page.screenshot({ path: shot("orchestrator-host-local") });
  await toggle.click();
  const menu = page.locator(".orch-rail .orch-host-menu");
  report.hostOptions = await menu.getByRole("menuitemradio").allInnerTexts();
  await menu.getByRole("menuitemradio", { name: /^Build box/ }).click();
  // The seeded task's repo is offered first, so the panel opens on it.
  const row = page.locator(".ui-task-row", {
    hasText: "Add rate limiting to the API",
  });
  await row.first().waitFor({ timeout: 60000 });
  report.hostBar = (await toggle.innerText()).trim();
  await row.first().click();
  await page.locator(".td-head").waitFor();
  await page.screenshot({ path: shot("orchestrator-host-selector") });
  report.screenshots = [
    "orchestrator-host-local",
    "orchestrator-host-selector",
  ];
} catch (error) {
  report.error = String(error?.stack ?? error);
  process.exitCode = 1;
} finally {
  await app?.close().catch(() => {});
  // The remote daemon outlives the app by design; this run stops its own.
  try {
    const pid = Number(
      await fs.readFile(
        `${profile}/remote-home/.sushiai/orchestrator/orchd.pid`,
        "utf8",
      ),
    );
    process.kill(pid, "SIGTERM");
  } catch {
    // Never started.
  }
  await fs
    .rm(profile, {
      recursive: true,
      force: true,
      maxRetries: 10,
      retryDelay: 100,
    })
    .catch(() => {});
  console.log(JSON.stringify(report, null, 2));
}
