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
    const { installTerminalInteractions, cleanTerminalCopy } =
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
    const streamed = make();
    const remoteEvents = [],
      remoteInput = [],
      selectionChanges = [];
    const remoteCleanup = installTerminalInteractions(
      streamed,
      streamed.element.parentElement,
      (data) => remoteInput.push(data),
      (direction, lines, position) =>
        remoteEvents.push({ direction, lines, position }),
      (active) => selectionChanges.push(active),
    );
    await write(streamed, "\x1b[2J\x1b[H  Jump to bottom (click) ↓");
    const remoteScreen = streamed.element.querySelector(".xterm-screen");
    const bounds = remoteScreen.getBoundingClientRect();
    const point = {
      clientX: bounds.left + (bounds.width * 10.5) / streamed.cols,
      clientY: bounds.top + (bounds.height * 0.5) / streamed.rows,
    };
    remoteScreen.dispatchEvent(
      new MouseEvent("mousedown", {
        ...point,
        button: 0,
        bubbles: true,
        cancelable: true,
      }),
    );
    remoteScreen.dispatchEvent(
      new WheelEvent("wheel", {
        ...point,
        deltaY: -20,
        bubbles: true,
        cancelable: true,
      }),
    );
    remoteScreen.dispatchEvent(
      new WheelEvent("wheel", {
        ...point,
        deltaY: -20,
        altKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    const idleWheelSelectionChanges = selectionChanges.length;
    streamed.focus();
    streamed.selectAll();
    await write(streamed, "\x1b[2J\x1b[HNEW FRAME");
    const remoteClipboard = new DataTransfer();
    streamed.textarea.dispatchEvent(
      new ClipboardEvent("copy", {
        clipboardData: remoteClipboard,
        bubbles: true,
        cancelable: true,
      }),
    );
    const copyMatchesVisibleFrame =
      remoteClipboard.getData("text/plain") ===
      cleanTerminalCopy(streamed.getSelection());
    remoteCleanup();
    return {
      remoteInput,
      remoteEvents,
      idleWheelSelectionChanges,
      copyMatchesVisibleFrame,
      shortcuts,
      normalScroll,
      fastScroll,
      mouseReports: countReports(mouseReports),
      fastMouseReports: countReports(fastMouseReports),
      copyMatches: copied === cleanTerminalCopy(interactive.getSelection()),
      cleanCopy: cleanTerminalCopy("  code    \n    nested  x \t\r\n"),
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
  assert.equal(results.cleanCopy, "  code\n    nested  x\n");
  assert.equal(results.idleWheelSelectionChanges, 0,
    "Scrolling without a selection must not trigger selection notifications or resize work");
  assert.deepEqual(results.remoteInput, ["\x1b[<0;11;1M\x1b[<0;11;1m"]);
  assert.deepEqual(results.remoteEvents, [
    { direction: "up", lines: 1, position: { column: 10, row: 0 } },
    { direction: "up", lines: 1, position: { column: 10, row: 0, fast: true } },
  ]);
  assert.equal(
    results.copyMatchesVisibleFrame,
    true,
    "Copy must read the current visible selection, never a stale saved string",
  );
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
    () => window.terminalHarness.calls.writes.length === 1,
  );
  let calls = await page.evaluate(() => window.terminalHarness.calls);
  assert.deepEqual(calls.files, ["image one.png"]);
  assert.equal(calls.drops, 0, "file drops must not reach panel rearrangement");
  assert.equal(
    calls.writes[0],
    "\x1b[200~'/tmp/attachments/image one.png' \x1b[201~",
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
    () => window.terminalHarness.calls.writes.length === 2,
  );
  assert.equal(
    await page
      .locator(".xterm-helper-textarea")
      .evaluate((el) => el === document.activeElement),
    true,
  );
  // Hold an actual mouse selection while Herdr redraws its full screen.
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
  assert.equal(await page.locator(".terminal-herdr .scrollbar.vertical").evaluateAll(
    (elements) => elements.every((element) => getComputedStyle(element).display === "none")), true);
  await page.mouse.move(grid.x + 200, cell.y + cell.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(100);
  assert.equal(await page.locator(".terminal-selection-status").count(), 0,
    "Holding a click without selecting text must not show a selection indicator");
  await page.mouse.up();
  // Double click chooses the real word under the pointer, including Cyrillic.
  await page.mouse.dblclick(grid.x + 4 * 7.2, cell.y + cell.height / 2);
  assert.equal(await page.locator(".terminal-selection-status").count(), 0);
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
  await page.evaluate(() =>
    window.terminalHarness.output("\x1b[2J\x1b[HNEW LIVE FRAME"),
  );
  await page.waitForTimeout(150);
  assert.equal(await readCopy(), selectedText);
  assert.ok(
    await page
      .locator(".xterm-rows")
      .textContent()
      .then((text) => text.includes("Привет")),
  );
  await page.screenshot({ path: "artifacts/terminal-selection.png" });
  await page.mouse.click(grid.x + 600, cell.y + cell.height * 4.5);
  await page.waitForFunction(() =>
    document
      .querySelector(".xterm-rows")
      ?.textContent.includes("NEW LIVE FRAME"),
  );
  assert.equal(await page.locator(".terminal-selection-status").count(), 0);
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
  await page.evaluate(() =>
    window.terminalHarness.output("\x1b[2J\x1b[HFRAME DURING DRAG"),
  );
  await page.mouse.move(grid.x + 400, cell.y + cell.height * 1.5, { steps: 5 });
  await page.mouse.up();
  assert.equal(await readCopy(), "alpha first line\nbeta second line");
  await page.evaluate(
    () => (document.getElementById("test-root").style.width = "700px"),
  );
  await page.waitForTimeout(150);
  assert.equal(await readCopy(), "alpha first line\nbeta second line");
  await page.mouse.click(grid.x + 600, cell.y + cell.height * 4.5);
  await page.waitForFunction(() =>
    document
      .querySelector(".xterm-rows")
      ?.textContent.includes("FRAME DURING DRAG"),
  );
  const firstSentence =
    "  ⏺ Теперь перезапускаю обучение раунда r1 с чекпоинтами (та же выборка, тот же сид — только теперь сохраняем промежуточные шаги вместо";
  await page.evaluate(
    (first) =>
      window.terminalHarness.output(
        "\x1b[2J\x1b[H" + first + "\r\n  одного финального).",
      ),
    firstSentence,
  );
  await page.waitForFunction(() =>
    document.querySelector(".xterm-rows")?.textContent.includes("финального"),
  );
  await page.mouse.move(grid.x + 0.1, cell.y + cell.height / 2);
  await page.mouse.down();
  await page.mouse.move(grid.x + 500, cell.y + cell.height * 2.5, { steps: 5 });
  await page.mouse.up();
  assert.equal(
    (await readCopy()).trimEnd(),
    firstSentence + " одного финального).",
  );
  await page.mouse.click(grid.x + 600, cell.y + cell.height * 4.5);
  // A selected transcript pauses Herdr frames. Jump must release that pause,
  // otherwise Claude moves to the bottom but the user keeps seeing the old frame.
  await page.evaluate(() => window.terminalHarness.output(
    "\x1b[2J\x1b[HSelect this transcript\r\n  Jump to bottom (click) ↓",
  ));
  await page.waitForFunction(() => document.querySelector(".xterm-rows")?.textContent.includes("Jump to bottom"));
  await page.mouse.dblclick(grid.x + 4 * 7.2, cell.y + cell.height / 2);
  await page.evaluate(() => window.terminalHarness.output("\x1b[2J\x1b[HQUEUED FRAME"));
  await page.waitForTimeout(100);
  assert.ok((await page.locator(".xterm-rows").textContent()).includes("Jump to bottom"));
  const beforeJump = await page.evaluate(() => window.terminalHarness.calls.writes.length);
  await page.mouse.click(grid.x + 10 * 7.2, cell.y + cell.height * 1.5);
  await page.waitForFunction(() => document.querySelector(".xterm-rows")?.textContent.includes("QUEUED FRAME"));
  assert.equal(await readCopy(), "", "Jump clears the selection that paused rendering");
  const jumpWrites = await page.evaluate(before => window.terminalHarness.calls.writes.slice(before), beforeJump);
  assert.equal(jumpWrites.length, 1);
  assert.match(jumpWrites[0], /^\x1b\[<0;\d+;2M\x1b\[<0;\d+;2m$/);
  await page.evaluate(() => window.terminalHarness.output("\x1b[2J\x1b[HACTUAL BOTTOM FRAME"));
  await page.waitForFunction(() => document.querySelector(".xterm-rows")?.textContent.includes("ACTUAL BOTTOM FRAME"));
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
    calls.writes.length,
    3,
    "failed attachment must not insert a path",
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
  // Returning from a harness to zsh immediately disables all file attachments.
  await page.evaluate(() => window.terminalHarness.shell());
  const fileCount = await page.evaluate(() => window.terminalHarness.calls.files.length);
  await drop();
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File(["image"], "blocked.png", { type: "image/png" }));
    document.querySelector(".xterm-helper-textarea").dispatchEvent(
      new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }),
    );
  });
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.terminalHarness.calls.files.length), fileCount);
  assert.equal(await page.evaluate(async () => {
    const fonts = await document.fonts.load('12px "Sushi Terminal Symbols"', "󰂺");
    return fonts.length > 0 && fonts.every(font => font.status === "loaded");
  }), true, "Bundled terminal symbols must load without installed Nerd Fonts");
  await page.evaluate(() => window.terminalHarness.dispose());
  console.log(
    "Component checks passed: drop, paste, focus, error dismissal, bracketed input.",
  );
} finally {
  await browser?.close();
  await server.close();
}
