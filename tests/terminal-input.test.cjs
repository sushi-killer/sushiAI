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
