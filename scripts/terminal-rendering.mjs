import { chromium } from "playwright";
import { createServer } from "vite";
import assert from "node:assert/strict";

const server = await createServer({
  server: { port: 5189, strictPort: false, hmr: false },
});
await server.listen();
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.TERMINAL_TEST_BROWSER || undefined,
  });
  const page = await browser.newPage();
  page.setDefaultTimeout(10000);
  page.on("console", (message) => {
    if (message.type() === "error") console.error(message.text());
  });
  page.on("requestfailed", (request) =>
    console.error(request.url(), request.failure()),
  );
  page.on("pageerror", (error) => console.error(error.message));
  const dialogs = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    dialog.dismiss().catch(() => {});
  });
  await page.route("**/terminal-test.html", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<html><body><div id="host" style="width:800px;height:500px"></div></body></html>',
    }),
  );
  await page.goto(`${server.resolvedUrls.local[0]}terminal-test.html`);
  const results = await page.evaluate(async () => {
    const { Terminal } =
      await import("/node_modules/@xterm/xterm/lib/xterm.mjs");
    const { terminalDimensions, queueTerminalFit } =
      await import("/src/terminal-sizing.ts");
    const make = () => {
      const host = document.createElement("div");
      document.body.append(host);
      const terminal = new Terminal({
        cols: 80,
        rows: 24,
        allowProposedApi: true,
      });
      terminal.open(host);
      return terminal;
    };
    const write = (terminal, data) =>
      new Promise((resolve) => terminal.write(data, resolve));
    const snapshot = (terminal) =>
      Array.from({ length: terminal.buffer.active.length }, (_, i) =>
        terminal.buffer.active.getLine(i).translateToString(true),
      );
    const message = "USER_MESSAGE_" + "abcdefghij ".repeat(30) + "_END";
    const frame = "\x1b[2J\x1b[H" + message + "\r\n\x1b[20;1HSTATUS\r\n";
    const expected = make(),
      fixed = make(),
      old = make();
    await write(expected, frame);
    expected.resize(30, 8);
    old.write(frame);
    old.resize(30, 8);
    await write(old, "");
    fixed.write(frame);
    const events = [];
    await new Promise((resolve) =>
      queueTerminalFit(
        fixed,
        { proposeDimensions: () => ({ cols: 30, rows: 8 }) },
        () => true,
        () => {
          events.push([fixed.cols, fixed.rows]);
          resolve();
        },
      ),
    );
    const equivalent =
      JSON.stringify(snapshot(fixed)) === JSON.stringify(snapshot(expected));
    const oldDiffers =
      JSON.stringify(snapshot(old)) !== JSON.stringify(snapshot(expected));
    // A complete long message survives repeated narrow/wide layouts.
    for (let i = 0; i < 10; i++) {
      fixed.resize(80, 24);
      fixed.resize(30, 8);
    }
    const retained = snapshot(fixed).join("").includes(message);
    const before = [fixed.cols, fixed.rows];
    queueTerminalFit(
      fixed,
      { proposeDimensions: () => ({ cols: 2, rows: 1 }) },
      () => false,
      () => events.push("unexpected hidden resize"),
    );
    await write(fixed, "");
    const { installTerminalInteractions } =
      await import("/src/terminal-interactions.ts");
    const interactive = make();
    const input = [];
    const cleanup = installTerminalInteractions(
      interactive,
      interactive.element.parentElement,
      (data) => input.push(data),
    );
    interactive.onData((data) => input.push(data));
    const textarea = interactive.textarea;
    for (const init of [
      { key: "Backspace", metaKey: true },
      { key: "ArrowLeft", metaKey: true },
      { key: "ArrowRight", metaKey: true },
      { key: "Backspace", altKey: true },
      { key: "ArrowLeft", altKey: true },
      { key: "ArrowRight", altKey: true },
      { key: "Enter", shiftKey: true },
    ]) {
      for (const type of ["keydown", "keyup"])
        textarea.dispatchEvent(
          new KeyboardEvent(type, { ...init, bubbles: true, cancelable: true }),
        );
    }
    const shortcuts = input.splice(0);
    await write(
      interactive,
      Array.from({ length: 500 }, (_, i) => `line ${i}\r\n`).join(""),
    );
    const screen = interactive.element.querySelector(".xterm-screen");
    const rect = screen.getBoundingClientRect();
    const wheel = (altKey) =>
      screen.dispatchEvent(
        new WheelEvent("wheel", {
          deltaY: -20,
          deltaMode: 0,
          altKey,
          bubbles: true,
          cancelable: true,
          clientX: rect.left + 10,
          clientY: rect.top + 10,
        }),
      );
    interactive.scrollToBottom();
    const bottom = interactive.buffer.active.viewportY;
    wheel(false);
    const normalScroll = bottom - interactive.buffer.active.viewportY;
    interactive.scrollToBottom();
    wheel(true);
    const fastScroll = bottom - interactive.buffer.active.viewportY;
    await write(interactive, "\x1b[?1049h\x1b[?1000h\x1b[?1006h");
    input.length = 0;
    wheel(false);
    const mouseReports = input.splice(0).join("");
    wheel(true);
    const fastMouseReports = input.splice(0).join("");
    const countReports = (text) => (text.match(/\x1b\[</g) || []).length;
    await write(interactive, "\x1b[?1000l\x1b[?1049l");
    interactive.focus();
    interactive.selectAll();
    const clipboard = new DataTransfer();
    textarea.dispatchEvent(
      new ClipboardEvent("copy", {
        clipboardData: clipboard,
        bubbles: true,
        cancelable: true,
      }),
    );
    const copied = clipboard.getData("text/plain");
    cleanup();
    return {
      shortcuts,
      normalScroll,
      fastScroll,
      mouseReports: countReports(mouseReports),
      fastMouseReports: countReports(fastMouseReports),
      copyMatches: copied === interactive.getSelection(),
      equivalent,
      oldDiffers,
      retained,
      events,
      hiddenUnchanged:
        JSON.stringify(before) === JSON.stringify([fixed.cols, fixed.rows]),
      minimum: terminalDimensions({ cols: 2, rows: 1 }),
      maximum: terminalDimensions({ cols: 900, rows: 600 }),
      invalid: terminalDimensions({ cols: NaN, rows: 10 }) === undefined,
    };
  });
  assert.equal(
    results.oldDiffers,
    true,
    "old resize order must reproduce corruption",
  );
  assert.equal(
    results.equivalent,
    true,
    "queued resize must match fully parsed output",
  );
  assert.equal(
    results.retained,
    true,
    "long user message must survive repeated resizing",
  );
  assert.deepEqual(results.shortcuts, [
    "\x15",
    "\x01",
    "\x05",
    "\x1b\x7f",
    "\x1bb",
    "\x1bf",
    "\x1b[13;2u",
  ]);
  assert.equal(results.copyMatches, true);
  assert.equal(results.hiddenUnchanged, true);
  assert.deepEqual(results.events, [[30, 8]]);
  assert.deepEqual(results.minimum, { cols: 10, rows: 3 });
  assert.deepEqual(results.maximum, { cols: 500, rows: 300 });
  assert.equal(results.invalid, true);
  console.log(JSON.stringify(results, null, 2));
  await page.addScriptTag({
    type: "module",
    content: `
    import('/tests/terminal-harness.tsx');
  `,
  });
  await page.waitForSelector(".terminal-surface").catch(async (error) => {
    console.error(await page.locator("body").innerHTML());
    throw error;
  });
  await page.waitForTimeout(100);
  const drop = () =>
    page.evaluate(() => {
      const data = new DataTransfer();
      data.items.add(
        new File([new Uint8Array([137, 80, 78, 71])], "image one.png", {
          type: "image/png",
        }),
      );
      const surface = document.querySelector(".terminal-surface");
      surface.dispatchEvent(
        new DragEvent("dragover", {
          dataTransfer: data,
          bubbles: true,
          cancelable: true,
        }),
      );
      surface.dispatchEvent(
        new DragEvent("drop", {
          dataTransfer: data,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
  await drop();
  await page.waitForFunction(
    () => window.terminalHarness.calls.files.length === 1,
  );
  let calls = await page.evaluate(() => window.terminalHarness.calls);
  assert.deepEqual(calls.files, ["image one.png"]);
  assert.equal(calls.drops, 0, "file drops must not reach panel rearrangement");
  assert.deepEqual(
    calls.writes,
    [],
    "the daemon writes the path of an attachment, the renderer sends nothing",
  );
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(["image"], "pasted.png", { type: "image/png" }));
    document.querySelector(".xterm-helper-textarea").dispatchEvent(
      new ClipboardEvent("paste", {
        clipboardData: data,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await page.waitForFunction(
    () => window.terminalHarness.calls.files.length === 2,
  );
  assert.equal(
    await page
      .locator(".xterm-helper-textarea")
      .evaluate((el) => el === document.activeElement),
    true,
  );
  // Hold an actual mouse selection while the program redraws its full screen.
  await page.evaluate(() =>
    window.terminalHarness.output(
      "\x1b[2J\x1b[H  Привет selected text here\r\nsecond line",
    ),
  );
  await page.waitForFunction(() =>
    document.querySelector(".xterm-rows")?.textContent.includes("Привет"),
  );
  const grid = await page.locator(".xterm-screen").boundingBox();
  const cell = await page.locator(".xterm-rows > div").first().boundingBox();
  assert.equal(
    await page
      .locator(".terminal-surface .scrollbar.vertical")
      .evaluateAll((elements) =>
        elements.every(
          (element) => getComputedStyle(element).display === "none",
        ),
      ),
    true,
  );
  await page.mouse.move(grid.x + 200, cell.y + cell.height / 2);
  await page.mouse.down();
  await page.mouse.up();
  // Double click chooses the real word under the pointer, including Cyrillic.
  await page.mouse.dblclick(grid.x + 4 * 7.2, cell.y + cell.height / 2);
  const readCopy = () =>
    page.evaluate(() => {
      const clipboard = new DataTransfer();
      document.querySelector(".xterm-helper-textarea").dispatchEvent(
        new ClipboardEvent("copy", {
          clipboardData: clipboard,
          bubbles: true,
          cancelable: true,
        }),
      );
      return clipboard.getData("text/plain");
    });
  const selectedText = await readCopy();
  assert.equal(selectedText, "Привет");
  await page.screenshot({ path: "artifacts/terminal-selection.png" });
  await page.mouse.click(grid.x + 600, cell.y + cell.height * 4.5);
  await page.evaluate(() =>
    window.terminalHarness.output(
      "\x1b[2J\x1b[Halpha first line\r\nbeta second line",
    ),
  );
  await page.waitForFunction(() =>
    document.querySelector(".xterm-rows")?.textContent.includes("alpha first"),
  );
  await page.mouse.move(grid.x + 0.1, cell.y + cell.height / 2);
  await page.mouse.down();
  await page.mouse.move(grid.x + 400, cell.y + cell.height * 1.5, { steps: 5 });
  await page.mouse.up();
  assert.equal(await readCopy(), "alpha first line\nbeta second line");
  await page.evaluate(
    () => (document.getElementById("test-root").style.width = "700px"),
  );
  await page.waitForTimeout(150);
  assert.equal(await readCopy(), "alpha first line\nbeta second line");
  await page.mouse.click(grid.x + 600, cell.y + cell.height * 4.5);
  // Links: plain URL and OSC 8 open on Cmd/Ctrl+click only, never a dialog.
  await page.evaluate(() =>
    window.terminalHarness.output(
      "\x1b[2J\x1b[Hsee https://example.com/plain now\r\n" +
        "\x1b]8;;https://example.com/osc\x07label\x1b]8;;\x07\r\n" +
        "\x1b]8;;file:///etc/passwd\x07local\x1b]8;;\x07\r\n",
    ),
  );
  await page.waitForFunction(() =>
    document.querySelector(".xterm-rows")?.textContent.includes("example.com"),
  );
  const modifier = process.platform === "darwin" ? "Meta" : "Control";
  const rowY = (row) => cell.y + cell.height * (row + 0.5);
  const opened = () => page.evaluate(() => window.terminalHarness.calls.opened);
  const clickWith = async (x, y) => {
    await page.mouse.move(x, y);
    await page.waitForTimeout(150);
    await page.keyboard.down(modifier);
    await page.mouse.click(x, y);
    await page.keyboard.up(modifier);
    await page.waitForTimeout(100);
  };
  await page.mouse.move(grid.x + 5 * 7.2, rowY(0));
  await page.mouse.move(grid.x + 12 * 7.2, rowY(0));
  await page.waitForFunction(() =>
    document
      .querySelector(".xterm-screen")
      ?.classList.contains("xterm-cursor-pointer"),
  );
  await page.screenshot({ path: "artifacts/terminal-link-hover.png" });
  await page.mouse.click(grid.x + 12 * 7.2, rowY(0));
  await page.waitForTimeout(100);
  assert.deepEqual(await opened(), [], "a plain click must not open a link");
  await clickWith(grid.x + 12 * 7.2, rowY(0));
  assert.deepEqual(await opened(), ["https://example.com/plain"]);
  await clickWith(grid.x + 2 * 7.2, rowY(1));
  assert.deepEqual(await opened(), [
    "https://example.com/plain",
    "https://example.com/osc",
  ]);
  await clickWith(grid.x + 2 * 7.2, rowY(2));
  assert.equal((await opened()).length, 2, "file: links must never open");
  assert.deepEqual(dialogs, [], "links must not show any dialog");
  await page.evaluate(() => window.terminalHarness.fail());
  await drop();
  await page.getByRole("button", { name: "Dismiss", exact: true }).click();
  calls = await page.evaluate(() => window.terminalHarness.calls);
  assert.equal(
    calls.closed,
    0,
    "dismissing an attachment error must preserve the session",
  );
  assert.equal(
    calls.files.length,
    3,
    "the failed attachment was still attempted once",
  );
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(["preview"], "preview.png", { type: "image/png" }));
    document.querySelector(".terminal-surface").dispatchEvent(
      new DragEvent("dragover", {
        dataTransfer: data,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await page.screenshot({ path: "artifacts/terminal-drop.png" });
  assert.equal(
    await page.evaluate(async () => {
      const fonts = await document.fonts.load(
        '12px "Sushi Terminal Symbols"',
        "󰂺",
      );
      return (
        fonts.length > 0 && fonts.every((font) => font.status === "loaded")
      );
    }),
    true,
    "Bundled terminal symbols must load without installed Nerd Fonts",
  );
  await page.evaluate(() => window.terminalHarness.dispose());
  console.log(
    "Component checks passed: drop, paste, focus, error dismissal, bracketed input.",
  );
} finally {
  await browser?.close();
  await server.close();
}
