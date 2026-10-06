// Screenshot recipe for the desktop mascot: needs-input, done, failed, the
// stack with its pager, the pill and the dev core-update bubble.
//   npm run build && node .agents/skills/ui-evidence/scripts/mascot.mjs [outDir] [prefix]
// Pushes synthetic notices through the SUSHIAI_TEST_MASCOT seam (no daemon, no
// network), photographs the mascot window and writes <outDir>/<prefix>-*.png.
// Uses a throwaway profile; never the owner's profile or the dev server port.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import { stopDaemon } from "../../../../scripts/lib/daemon-binary.mjs";

const root = process.cwd();
const outDir = process.argv[2] ?? `${root}/artifacts`;
const prefix = process.argv[3] ?? "mascot";
const shot = (name) => `${outDir}/${prefix}-${name}.png`;
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const report = { pageErrors: [] };
let app = null;

const TASKS = {
  input: {
    id: "11111111-1111-4111-8111-111111111111",
    title: "Tidy the export path",
    status: "waiting",
    repo: "/work/evidence-repo",
    costUsd: 0.31,
    question: {
      text: "Delete the legacy export path, or keep it behind a flag? The old path has no callers in this repo.",
      options: ["Delete it", "Keep behind a flag", "Stop"],
      askedBy: "verify",
      askedAt: Date.now(),
    },
  },
  done: {
    id: "22222222-2222-4222-8222-222222222222",
    title: "Landed feature",
    status: "done",
    repo: "/work/evidence-repo",
    costUsd: 1.42,
    branch: "task/landed-feature",
    baseRef: "main",
    updatedAt: Date.now(),
    attempts: [{ review: { verdict: "PASS" } }],
  },
  failed: {
    id: "33333333-3333-4333-8333-333333333333",
    title: "Verify broke",
    status: "failed",
    repo: "/work/evidence-repo",
    costUsd: 0.87,
    updatedAt: Date.now(),
    attempts: [{ failure: { kind: "verify" } }],
  },
};

async function mascotWindow() {
  for (let i = 0; i < 100; i += 1) {
    const found = app.windows().find((w) => w.url().includes("mascot.html"));
    if (found) return found;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("the mascot window never appeared");
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 900));

try {
  await fs.mkdir(outDir, { recursive: true });
  app = await electron.launch({
    args: ["."],
    cwd: root,
    env: {
      ...process.env,
      BRIDGE_DATA_DIR: profile,
      SUSHIAI_HOME: `${profile}/sushiai`,
      BRIDGE_DEV_URL: "",
      SUSHIAI_TEST_MASCOT: "1",
      SUSHIAI_TEST_WINDOW: "hidden",
    },
  });
  await app.firstWindow();
  const push = (task) =>
    app.evaluate(
      (_electron, value) => globalThis.__sushiaiMascot.notify(value),
      task,
    );

  await push(TASKS.done);
  const mascot = await mascotWindow();
  mascot.on("pageerror", (error) => report.pageErrors.push(error.message));
  await mascot.locator(".bubble.done").waitFor({ timeout: 10000 });
  await settle();
  await mascot.screenshot({ path: shot("done") });

  await push(TASKS.failed);
  await mascot.locator(".bubble.failed").waitFor({ timeout: 10000 });
  await settle();
  await mascot.screenshot({ path: shot("failed") });

  await push(TASKS.input);
  await mascot.locator(".bubble.input").waitFor({ timeout: 10000 });
  await settle();
  await mascot.screenshot({ path: shot("input") });

  await app.evaluate(() => globalThis.__sushiaiMascot.toggle());
  await mascot.locator(".pill").waitFor({ timeout: 10000 });
  await settle();
  await mascot.screenshot({ path: shot("pill") });
  await app.evaluate(() => globalThis.__sushiaiMascot.toggle());
  await mascot.locator(".bubble-wrap").waitFor({ timeout: 10000 });

  await app.evaluate(() => globalThis.__sushiaiMascot.coreUpdated());
  await mascot.locator(".bubble.info").waitFor({ timeout: 10000 });
  await settle();
  await mascot.screenshot({ path: shot("core-update") });
} catch (error) {
  report.error = String(error?.message ?? error);
} finally {
  if (app) await app.close().catch(() => {});
  stopDaemon(`${profile}/sushiai`);
  await fs.rm(profile, { recursive: true, force: true });
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.pageErrors.length > 0) process.exitCode = 1;
}
