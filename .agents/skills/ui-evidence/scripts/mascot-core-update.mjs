// Screenshot recipe for the dev core-update notice on the desktop mascot.
//   npm run build && node .agents/skills/ui-evidence/scripts/mascot-core-update.mjs && open artifacts/mascot-core-update.png
// Calls the SUSHIAI_TEST_MASCOT seam's coreUpdated() twice (the queue must keep
// one entry, with no expiry), screenshots the bubble, clicks Restart and waits
// for Electron to exit. Writes artifacts/mascot-core-update.json with
// { queueLength, expiresAt, buttons, exitCode }; exits non-zero unless
// queueLength is 1, expiresAt is null and exitCode is 75.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";

const root = process.cwd();
const report = { pageErrors: [] };
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
let app = null;

try {
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
  const proc = app.process();
  const exited = new Promise((resolve) =>
    proc.once("exit", (code) => resolve(code)),
  );
  await app.firstWindow();
  for (let i = 0; i < 2; i += 1)
    await app.evaluate(() => globalThis.__sushiaiMascot.coreUpdated());

  let mascot = null;
  for (let i = 0; i < 100 && !mascot; i += 1) {
    mascot = app.windows().find((w) => w.url().includes("mascot.html"));
    if (!mascot) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  if (!mascot) throw new Error("the mascot window never appeared");
  mascot.on("pageerror", (error) => report.pageErrors.push(error.message));
  await mascot
    .locator(".bubble", { hasText: "Core updated - restart?" })
    .waitFor({ timeout: 10000 });
  await new Promise((resolve) => setTimeout(resolve, 700));

  const queue = await app.evaluate(() => globalThis.__sushiaiMascot.queue());
  report.queueLength = queue.length;
  report.expiresAt = queue.length ? queue[0].expiresAt : "missing";
  report.buttons = (await mascot.getByRole("button").allInnerTexts())
    .concat(
      (await mascot.locator("button[aria-label]").all()).length
        ? await mascot
            .locator("button[aria-label]")
            .evaluateAll((els) => els.map((el) => el.ariaLabel))
        : [],
    )
    .filter(Boolean);
  await mascot.screenshot({ path: `${root}/artifacts/mascot-core-update.png` });

  await mascot.getByRole("button", { name: "Restart" }).click();
  report.exitCode = await Promise.race([
    exited,
    new Promise((resolve) => setTimeout(() => resolve("timeout"), 20000)),
  ]);
  app = null;

  const problems = [];
  if (report.queueLength !== 1) problems.push("queueLength is not 1");
  if (report.expiresAt !== null) problems.push("expiresAt is not null");
  if (report.exitCode !== 75) problems.push("exitCode is not 75");
  if (problems.length) report.error = problems.join("; ");
} catch (error) {
  report.error = String(error?.message ?? error);
} finally {
  if (app) await app.close().catch(() => {});
  await fs.rm(profile, { recursive: true, force: true });
  await fs
    .writeFile(
      `${root}/artifacts/mascot-core-update.json`,
      JSON.stringify(report, null, 2),
    )
    .catch(() => {});
  console.log(JSON.stringify(report, null, 2));
  if (report.error || report.pageErrors.length > 0) process.exitCode = 1;
}
