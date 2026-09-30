import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import assert from "node:assert/strict";
const profile = await fs.mkdtemp("/tmp/sushiai-tabs-test-");
// The app restores its workspace from this file on the first paint.
await fs.writeFile(
  `${profile}/workspace-state.json`,
  JSON.stringify({
    activeId: "test",
    socket: "/tmp/absent-herdr-test.sock",
    routines: [],
    fontScale: 1,
    workspaces: [
      {
        id: "test",
        name: "Keyboard test",
        cwd: "/tmp",
        panels: [
          { id: "one", title: "One", kind: "chat" },
          { id: "two", title: "Two", kind: "chat" },
        ],
        layout: {
          type: "split",
          id: "split",
          axis: "row",
          ratio: 0.5,
          a: { type: "leaf", id: "one" },
          b: { type: "leaf", id: "two" },
        },
      },
    ],
  }),
);
const app = await electron.launch({
  ...(process.env.SUSHIAI_EXECUTABLE
    ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
    : {}),
  args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
  env: {
    ...process.env,
    SUSHIAI_TEST_WINDOW: "hidden",
    BRIDGE_DATA_DIR: profile,
  },
});
try {
  const page = await app.firstWindow();
  await page.waitForSelector(".panel");
  await page.getByTitle("Switch to tabs", { exact: true }).click();
  await page.keyboard.press("Meta+Shift+BracketRight");
  assert.equal(
    await page
      .getByRole("tab", { name: "Two", exact: true })
      .getAttribute("aria-selected"),
    "true",
  );
  await page.keyboard.press("Meta+Shift+BracketLeft");
  assert.equal(
    await page
      .getByRole("tab", { name: "One", exact: true })
      .getAttribute("aria-selected"),
    "true",
  );
  await page.keyboard.press("Meta+Enter");
  assert.equal(
    await page.locator(".workspace-canvas .panel:visible").count(),
    1,
  );
  await page.keyboard.press("Escape");
  await page.keyboard.press("Meta+w");
  await page
    .getByRole("tab", { name: "One", exact: true })
    .waitFor({ state: "detached" });
  await page.keyboard.press("Meta+t");
  await page
    .getByRole("heading", { name: "Add a panel", exact: true })
    .waitFor();
  console.log(
    "PASS: previous/next tab, maximize, close tab and add-session shortcuts",
  );
} finally {
  await app.close();
  await fs.rm(profile, { recursive: true, force: true });
}
