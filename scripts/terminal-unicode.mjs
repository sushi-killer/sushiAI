import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import assert from "node:assert/strict";
import { daemonBinary, stopDaemon } from "./lib/daemon-binary.mjs";
const profile = await fs.mkdtemp("/tmp/sushiai-unicode-");
await fs.writeFile(`${profile}/.zshrc`, "");
let desktop;
try {
  desktop = await electron.launch({
    ...(process.env.SUSHIAI_EXECUTABLE
      ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
      : {}),
    args: process.env.SUSHIAI_EXECUTABLE ? [] : ["."],
    cwd: process.cwd(),
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      BRIDGE_DATA_DIR: profile,
      // Never the owner's ~/.codex or ~/.sushiai/bin link.
      HOME: profile,
      CODEX_HOME: `${profile}/codex`,
      SUSHIAI_HOME: `${profile}/sushiai`,
      SUSHIAI_DAEMON_BIN: daemonBinary(),
      BRIDGE_DEV_URL: "",
      ZDOTDIR: profile,
      // A GUI launch carries no locale; the daemon supplies a UTF-8 one.
      LANG: "",
      LC_ALL: "",
      LC_CTYPE: "",
      SHELL: "/bin/zsh",
    },
  });
  const page = await desktop.firstWindow();
  await page.waitForFunction(() => !!window.bridge);
  assert.equal(
    await page.evaluate(async () => {
      const fonts = await document.fonts.load(
        '12px "Sushi Terminal Symbols"',
        "󰂺",
      );
      return (
        fonts.length > 0 && fonts.every((font) => font.status === "loaded")
      );
    }),
    true,
    "Packaged terminal symbols must load from the bundled font",
  );
  await page.waitForFunction(
    async () =>
      (await window.bridge.daemonStates()).some(
        (state) => state.host === "local" && state.state === "ready",
      ),
    null,
    { timeout: 30000 },
  );
  const session = await page.evaluate(async (cwd) => {
    window.unicodeOutput = "";
    window.bridge.onDaemonTerminal((event) => {
      if (event.panelId === "unicode-check")
        window.unicodeOutput += (event.snapshot || "") + (event.data || "");
    });
    const launched = await window.bridge.daemonSessionLaunch({
      host: "local",
      cwd,
      cols: 80,
      rows: 24,
      idempotencyKey: "unicode-check",
    });
    await window.bridge.daemonTerminalAttach({
      panelId: "unicode-check",
      host: "local",
      sessionId: launched.sessionId,
      cols: 80,
      rows: 24,
    });
    return launched;
  }, profile);
  await page.waitForTimeout(400);
  await page.evaluate(() =>
    window.bridge.daemonTerminalWrite(
      "unicode-check",
      "printf '\\nRESULT:%s\\n' Приве",
    ),
  );
  await page.evaluate(() =>
    window.bridge.daemonTerminalWrite("unicode-check", "тш"),
  );
  await page.evaluate(() =>
    window.bridge.daemonTerminalWrite("unicode-check", "\x7f\r"),
  );
  await page
    .waitForFunction(() => window.unicodeOutput.includes("RESULT:Привет\r\n"))
    .catch(async (error) => {
      console.error(
        "Unicode terminal output:",
        JSON.stringify(await page.evaluate(() => window.unicodeOutput)),
      );
      throw error;
    });
  const result = await page.evaluate(async ({ host, sessionId }) => {
    await window.bridge.daemonTerminalDetach("unicode-check");
    await window.bridge.sessionClose(host, sessionId, false);
    return { replacement: window.unicodeOutput.includes("�") };
  }, session);
  assert.equal(result.replacement, false);
  console.log(
    "Real zsh in a daemon session: Cyrillic input + backspace survives a parent without a locale.",
  );
} finally {
  await desktop?.close();
  stopDaemon(`${profile}/sushiai`);
  // A shell that exits late may still write its history into HOME.
  await fs.rm(profile, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 200,
  });
}
