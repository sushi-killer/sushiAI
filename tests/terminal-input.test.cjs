const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createTerminalInput } = require("../src/terminal-output.ts");

test("renderer limits pending IPC input and preserves paste and Unicode order", async () => {
  const sent = [];
  const errors = [];
  const callbacks = [];
  const input = createTerminalInput(
    (data) => {
      sent.push(data);
      return new Promise((resolve) => callbacks.push(resolve));
    },
    (message) => errors.push(message),
  );
  input.send("Привет 🌍");
  input.send("paste".repeat(180000));
  input.send("overflow".repeat(50000));
  assert.equal(errors.length, 1);
  assert.ok(input.stats.bytes <= 1024 * 1024);
  callbacks.shift()();
  await new Promise((resolve) => setImmediate(resolve));
  callbacks.shift()();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, ["Привет 🌍", "paste".repeat(180000)]);
  assert.equal(input.stats.bytes, 0);
});

test("a host that reconnects re-attaches a pane its disconnect ended", async () => {
  const { shouldReattach } = await import("../src/terminal-output.ts");
  const live = { ready: true, error: "", exited: false };
  assert.equal(shouldReattach(live, false), false);
  const failed = { ready: false, error: "no route", exited: false };
  assert.equal(shouldReattach(failed, false), true);
  const dropped = { ready: true, error: "Session ended.", exited: true };
  assert.equal(shouldReattach(dropped, false), true);
  // The panel itself ended: the session is gone, nothing to attach to.
  assert.equal(shouldReattach(dropped, true), false);
});

test("a large snapshot reaches xterm in slices over frames, cut on lines, all acked", async () => {
  const { createOutputBatcher, FRAME_BUDGET } =
    await import("../src/terminal-output.ts");
  const log = [];
  const acks = [];
  const frames = [];
  const batcher = createOutputBatcher({
    write: (data, done) => {
      log.push(data);
      done();
    },
    reset: () => log.push(null),
    ack: (bytes) => acks.push(bytes),
    schedule: (flush) => frames.push(flush),
  });
  const screen = Array.from(
    { length: 2000 },
    (_, i) =>
      `\x1b[3${i % 7}m[${i}]\x1b[0m café \u{1F600} ${"x".repeat(i % 40)}\r\n`,
  ).join("");
  batcher.snapshot(screen);
  batcher.data("after");
  let flushed = 0;
  while (frames.length) {
    const before = log.length;
    frames.shift()();
    flushed += 1;
    const sizes = log.slice(before).filter((data) => data !== null);
    assert.ok(
      sizes.reduce((total, data) => total + data.length, 0) <= FRAME_BUDGET,
    );
  }
  assert.ok(flushed > 1, "the snapshot is spread over frames");
  assert.equal(log[0], null, "the reset comes first");
  const writes = log.slice(1);
  assert.equal(writes.join(""), screen + "after");
  for (const data of writes.slice(0, -1))
    assert.ok(data.endsWith("\n"), "cut after a line break");
  assert.equal(
    acks.reduce((total, n) => total + n, 0),
    Buffer.byteLength(screen + "after"),
  );
});
