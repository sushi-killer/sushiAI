const { test } = require("node:test");
const assert = require("node:assert/strict");
const load = () => import("../src/lib/text.ts");

test("plural reads singular for one and plural otherwise", async () => {
  const { plural } = await load();
  assert.equal(plural(1, "idle session"), "1 idle session");
  assert.equal(plural(2, "idle session"), "2 idle sessions");
  assert.equal(plural(0, "file"), "0 files");
});

test("elapsedLabel picks minutes, hours or days", async () => {
  const { elapsedLabel } = await load();
  assert.equal(elapsedLabel(2 * 60_000), "2m");
  assert.equal(elapsedLabel(90 * 60_000), "1h");
  assert.equal(elapsedLabel(50 * 3600_000), "2d");
  assert.equal(elapsedLabel(-5000), "0m");
});
