import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import assert from "node:assert/strict";
const profile = await fs.mkdtemp("/tmp/sushiai-unicode-");
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
      BRIDGE_DATA_DIR: profile,
      ZDOTDIR: profile,
      LANG: "C",
      LC_ALL: "C",
      SHELL: "/bin/zsh",
    },
  });
  const page = await desktop.firstWindow();
  await page.waitForFunction(() => !!window.bridge);
  assert.equal(await page.evaluate(async () => {
    const fonts = await document.fonts.load('12px "Sushi Terminal Symbols"', "󰂺");
    return fonts.length > 0 && fonts.every(font => font.status === "loaded");
  }), true, "Packaged terminal symbols must load from the bundled font");
  await page.evaluate(async (cwd) => {
    window.unicodeOutput = "";
    window.bridge.onTerminal((event) => {
      if (event.panelId === "unicode-check")
        window.unicodeOutput += event.data || "";
    });
    await window.bridge.terminalOpen({ panelId: "unicode-check", cwd });
  }, profile);
  await page.waitForTimeout(400);
  await page.evaluate(() =>
    window.bridge.terminalWrite(
      "unicode-check",
      "printf '\\nRESULT:%s\\n' Приве",
    ),
  );
  await page.evaluate(() => window.bridge.terminalWrite("unicode-check", "тш"));
  await page.evaluate(() =>
    window.bridge.terminalWrite("unicode-check", "\x7f\r"),
  );
  await page.waitForFunction(() =>
    window.unicodeOutput.includes("RESULT:Привет\r\n"),
  );
  const result = await page.evaluate(async () => {
    let blocked = false;
    try {
      await window.bridge.terminalAttach({
        panelId: "unicode-check",
        name: "image.png",
        data: btoa("image"),
      });
    } catch (error) {
      blocked = error.message.includes("active agent session");
    }
    await window.bridge.terminalClose("unicode-check");
    return { blocked, replacement: window.unicodeOutput.includes("\ufffd") };
  });
  assert.equal(result.blocked, true);
  assert.equal(result.replacement, false);
  console.log(
    "Real zsh: Cyrillic input + backspace survives ASCII parent locale; shell attachment rejected.",
  );
} finally {
  await desktop?.close();
  await fs.rm(profile, { recursive: true, force: true });
}
