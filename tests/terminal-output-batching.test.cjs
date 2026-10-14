const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  createOutputBatcher,
  createTerminalInput,
} = require("../src/terminal-output.ts");

// A fake xterm and frame clock: `frame()` runs the scheduled flushes and
// `parsed()` runs the write callbacks xterm calls once it has parsed the data.
function harness() {
  const log = [];
  const acks = [];
  const parsing = [];
  const frames = [];
  const batcher = createOutputBatcher({
    write: (data, done) => {
      log.push(["write", data]);
      parsing.push(done);
    },
    reset: () => log.push(["reset"]),
    ack: (bytes) => acks.push(bytes),
    schedule: (flush) => frames.push(flush),
  });
  return {
    batcher,
    log,
    acks,
    frames,
    frame: () => frames.splice(0).forEach((flush) => flush()),
    parsed: () => parsing.splice(0).forEach((done) => done()),
  };
}

const written = (log) =>
  log
    .filter(([kind]) => kind === "write")
    .map(([, data]) => data)
    .join("");
const sum = (values) => values.reduce((total, n) => total + n, 0);

test("output chunks that arrive within one frame reach the terminal as one write", () => {
  const t = harness();
  const lines = Array.from({ length: 1000 }, (_, i) => `line ${i}\r\n`);
  for (const line of lines) t.batcher.data(line);
  t.batcher.data("\u00e9\u{1F680}");
  assert.equal(t.log.length, 0, "nothing is written before the frame");
  assert.equal(t.frames.length, 1, "the whole burst schedules one frame");
  t.frame();
  const expected = lines.join("") + "\u00e9\u{1F680}";
  assert.deepEqual(t.log, [["write", expected]]);
  assert.deepEqual(t.acks, [], "bytes are acked once xterm has parsed them");
  t.parsed();
  assert.deepEqual(t.acks, [Buffer.byteLength(expected)]);
});

test("output after a flushed frame schedules the next frame", () => {
  const t = harness();
  t.batcher.data("a");
  t.frame();
  t.parsed();
  t.batcher.data("b");
  t.batcher.data("c");
  assert.equal(t.frames.length, 1);
  t.frame();
  t.parsed();
  assert.equal(written(t.log), "abc");
  assert.equal(t.log.length, 2);
  assert.equal(sum(t.acks), 3);
});

test("a snapshot replaces pending output, and the replaced bytes are still acked", () => {
  const t = harness();
  t.batcher.data("old output");
  t.batcher.snapshot("screen");
  t.batcher.data("\u00f1");
  for (let i = 0; i < 3; i++) {
    t.frame();
    t.parsed();
  }
  const kinds = t.log.map(([kind]) => kind);
  const reset = kinds.lastIndexOf("reset");
  assert.ok(reset >= 0, "the snapshot resets the terminal");
  assert.equal(written(t.log.slice(reset + 1)), "screen\u00f1");
  // Main counts every byte it sent as in flight until it is acked.
  assert.equal(
    sum(t.acks),
    Buffer.byteLength("old output") +
      Buffer.byteLength("screen") +
      Buffer.byteLength("\u00f1"),
  );
});

test("keystrokes go out without waiting for the previous keystroke's round trip, in order", async () => {
  const sent = [];
  const replies = [];
  const errors = [];
  const input = createTerminalInput(
    (data) => {
      sent.push(data);
      return new Promise((resolve) => replies.push(resolve));
    },
    (message) => errors.push(message),
  );
  for (const key of ["l", "s", " ", "-", "l", "\r"]) input.send(key);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    sent.join(""),
    "ls -l\r",
    "every key is on its way before any reply",
  );
  assert.ok(
    input.stats.bytes > 0,
    "bytes in flight still count against the cap",
  );
  replies.splice(0).forEach((resolve) => resolve());
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(input.stats.bytes, 0);
  assert.deepEqual(errors, []);
});
