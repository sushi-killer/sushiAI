import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import assert from "node:assert/strict";
const profile = await fs.mkdtemp("/tmp/sushiai-chat-test-");
const project = await fs.mkdtemp("/tmp/sushiai-chat-project-");
const desktop = await electron.launch({
  args: ["."],
  cwd: process.cwd(),
  env: { ...process.env, BRIDGE_DATA_DIR: profile },
});
try {
  const page = await desktop.firstWindow();
  await page.waitForSelector(".panel-agent");
  const result = await page.evaluate(async (cwd) => {
    const panelId = "sushiai-chat-integration";
    return await new Promise(async (resolve) => {
      let text = "";
      const timer = setTimeout(async () => {
        await window.bridge.cancelChat(panelId);
        resolve({ text, error: "Timed out after 60 seconds" });
      }, 60000);
      const unsubscribe = window.bridge.onChat((event) => {
        if (event.panelId !== panelId) return;
        if (event.text) text += event.text;
        if (event.done) {
          clearTimeout(timer);
          unsubscribe();
          resolve({ text, error: event.error });
        }
      });
      try {
        await window.bridge.chat({
          panelId,
          cwd,
          agent: "claude",
          messages: [
            {
              id: "test",
              role: "user",
              text: "Reply with exactly SUSHIAI_CHAT_OK. Do not use any tools, read files, or change anything.",
            },
          ],
        });
      } catch (error) {
        clearTimeout(timer);
        unsubscribe();
        resolve({ text, error: String(error) });
      }
    });
  }, project);
  assert.equal(result.error, undefined);
  assert.ok(result.text.includes("SUSHIAI_CHAT_OK"));
  console.log(
    "PASS: real Claude Code CLI request, stdin prompt, response delivery and process completion.",
  );
} finally {
  await desktop.close();
  await fs.rm(profile, { recursive: true, force: true });
  await fs.rm(project, { recursive: true, force: true });
}
