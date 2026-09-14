// UI evidence driver. Copy to artifacts/.driver.mjs, replace STEPS, run with
// node from the repo root, read every screenshot, then delete the copy.
import { _electron as electron } from "playwright";
import fs from "node:fs/promises";

const root = process.cwd();
const shot = (name) => `${root}/artifacts/${name}.png`;
const profile = await fs.mkdtemp("/tmp/sushiai-evidence-");
const app = await electron.launch({
  args: ["."],
  cwd: root,
  env: { ...process.env, BRIDGE_DATA_DIR: profile },
});
const report = { pageErrors: [] };
let sshId = null;

try {
  const page = await app.firstWindow();
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".panel-agent");

  // A second host comes from the shell (SUSHIAI_EVIDENCE_SSH=user@host), never
  // from a literal in a committed file.
  if (process.env.SUSHIAI_EVIDENCE_SSH) {
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const settings = page.getByRole("dialog", { name: "Settings" });
    await settings
      .getByRole("tab", { name: "Connections", exact: true })
      .click();
    await settings.getByRole("button", { name: "Add SSH host" }).click();
    const form = settings.locator("form.ssh-form");
    await form.locator("input[name=name]").fill("Remote");
    await form
      .locator("input[name=host]")
      .fill(process.env.SUSHIAI_EVIDENCE_SSH);
    await form
      .locator("input[name=socket]")
      .fill(
        process.env.SUSHIAI_EVIDENCE_SOCKET ?? "~/.config/herdr/herdr.sock",
      );
    await settings.getByRole("button", { name: "Save and connect" }).click();
    await settings
      .locator(".connection-card.selected")
      .filter({ hasText: "Remote" })
      .waitFor({ timeout: 30000 });
    const profiles = await page.evaluate(() =>
      window.bridge.connectionsList(),
    );
    sshId = profiles.find((item) => item.name === "Remote")?.id ?? null;
    await page.getByRole("button", { name: "Close dialog" }).click();
  }

  // STEPS - replace with the flow under test. The example measures the gap
  // between a workspace name's text ink and the tag after it.
  report.tagGaps = await page.evaluate(() =>
    [...document.querySelectorAll(".workspace-name")]
      .filter((row) => row.querySelector(":scope > .remote-tag"))
      .map((row) => {
        const name = row.querySelector(":scope > span");
        const ink = document.createRange();
        ink.selectNodeContents(name);
        const tag = row.querySelector(":scope > .remote-tag");
        return {
          name: name.textContent,
          gap: Math.round(
            tag.getBoundingClientRect().left - ink.getBoundingClientRect().right,
          ),
        };
      }),
  );
  await page.screenshot({ path: shot("evidence-window") });
  await page.locator(".sidebar").screenshot({ path: shot("evidence-sidebar") });
} catch (error) {
  report.error = String(error?.message ?? error);
} finally {
  if (sshId) {
    const page = await app.firstWindow();
    await page
      .evaluate(async (id) => {
        await window.bridge.connectionsDisconnect(`ssh:${id}`);
        await window.bridge.connectionsDelete(`ssh:${id}`);
      }, sshId)
      .catch((error) => (report.cleanupError = String(error)));
  }
  await app.close();
  await fs.rm(profile, { recursive: true, force: true });
  console.log(JSON.stringify(report, null, 2));
}
