const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  claudeForeground,
  scrollCommands,
} = require("../electron/terminal-stream.cjs");

test("Claude scrolling targets application input, with Alt acceleration and no host scroll", () => {
  assert.equal(
    claudeForeground({
      process_info: {
        foreground_processes: [{ name: "claude", argv0: "claude" }],
      },
    }),
    true,
  );
  assert.equal(
    claudeForeground({
      process_info: { foreground_processes: [{ name: "zsh", argv: ["-zsh"] }] },
    }),
    false,
  );
  const commands = scrollCommands(
    "up",
    6,
    { column: 12, row: 5, fast: true },
    true,
  );
  assert.equal(commands.length, 1);
  assert.equal(commands[0].type, "terminal.input");
  assert.equal(
    Buffer.from(commands[0].bytes, "base64").toString(),
    "\x1b[<64;13;6M".repeat(3),
  );
  assert.equal(
    scrollCommands("down", 6, { column: 0, row: 0 }, false)[0].type,
    "terminal.scroll",
  );
});

const { createScrollHandler } = require("../electron/terminal-stream.cjs");
const tick = () => new Promise((resolve) => setImmediate(resolve));
const reports = (commands) =>
  commands.reduce(
    (sum, command) =>
      sum +
      (
        Buffer.from(command.bytes || "", "base64")
          .toString()
          .match(/\x1b\[</g) || []
      ).length,
    0,
  );

test("warm scrolling has no process round trip and delivers exactly 30% more steps", async () => {
  const commands = [];
  let lookups = 0;
  const scroll = createScrollHandler({
    lookup: async () => {
      lookups++;
      return true;
    },
    write: (command) => commands.push(command),
    closed: () => false,
    now: () => 0,
  });
  await tick();
  for (let i = 0; i < 100; i++) {
    const before = commands.length;
    const sent = scroll("up", 1);
    assert.equal(
      commands.length,
      before + 1,
      "wheel is dispatched synchronously",
    );
    await sent;
  }
  assert.equal(lookups, 1);
  assert.equal(reports(commands), 130);
});

test("slow refresh never stalls scrolling; failures retain the Claude route", async () => {
  let now = 0,
    reject;
  const commands = [];
  let lookups = 0;
  const scroll = createScrollHandler({
    lookup: () =>
      ++lookups === 1
        ? Promise.resolve(true)
        : new Promise((_, fail) => {
            reject = fail;
          }),
    write: (command) => commands.push(command),
    closed: () => false,
    now: () => now,
  });
  await tick();
  now = 600;
  for (let i = 0; i < 10; i++) await scroll("up", 1);
  assert.equal(
    reports(commands),
    13,
    "all events sent while lookup is unresolved",
  );
  assert.equal(lookups, 2);
  reject(new Error("timeout"));
  await tick();
  await scroll("down", 1);
  assert.equal(commands.at(-1).type, "terminal.input");
});

test("route changes and input invalidation discard stale discovery results", async () => {
  let now = 0,
    resolve,
    foreground = true;
  const commands = [];
  let lookups = 0;
  const scroll = createScrollHandler({
    lookup: () =>
      ++lookups === 2
        ? new Promise((done) => {
            resolve = done;
          })
        : Promise.resolve(foreground),
    write: (command) => commands.push(command),
    closed: () => false,
    now: () => now,
  });
  await tick();
  now = 600;
  await scroll("up", 1);
  scroll.invalidate();
  foreground = false;
  await scroll("down", 6);
  assert.equal(commands.at(-1).type, "terminal.scroll");
  resolve(true);
  await tick();
  await scroll("down", 6);
  assert.equal(commands.at(-1).type, "terminal.scroll");
});

test("host history gains 30%, Alt remains three steps, and closed streams do not write", async () => {
  let closed = false;
  const commands = [];
  const scroll = createScrollHandler({
    lookup: async () => false,
    write: (command) => commands.push(command),
    closed: () => closed,
    now: () => 0,
  });
  for (let i = 0; i < 10; i++) await scroll("up", 6);
  assert.equal(commands.length, 13);
  assert.equal(
    commands.reduce((sum, command) => sum + command.lines, 0),
    78,
  );
  await scroll("up", 6, { fast: true });
  assert.equal(commands.length, 16);
  closed = true;
  await scroll("up", 6);
  assert.equal(commands.length, 16);
});
