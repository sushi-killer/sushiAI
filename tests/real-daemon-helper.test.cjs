const test = require("node:test");
const assert = require("node:assert/strict");
const { realDaemon } = require("./helpers/real-daemon.cjs");

test("a missing binary skips locally and fails under CI", () => {
  const none = () => false;
  const local = realDaemon({}, none);
  assert.equal(local.binary, undefined);
  assert.match(local.skip, /cargo build -p sushiai/);
  assert.throws(() => realDaemon({ CI: "true" }, none), /must not skip/);
});

test("a built binary is found and never skipped", () => {
  const found = realDaemon({ CI: "true" }, (file) =>
    file.endsWith("debug/sushiai"),
  );
  assert.match(found.binary, /debug\/sushiai$/);
  assert.equal(found.skip, false);
});
