const test = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const hook = path.join(
  __dirname,
  "..",
  "scripts",
  "hooks",
  "lesson-reminder.mjs",
);

test("the lesson reminder stays quiet inside an orchd task agent", () => {
  // Not JSON: without the early exit the hook would throw parsing it.
  const run = (env) =>
    spawnSync(process.execPath, [hook], {
      input: "not json",
      env: { ...process.env, SUSHIAI_LESSON_REMINDER_NESTED: "", ...env },
    });
  assert.equal(run({ ORCHD_TASK: "task-1" }).status, 0);
  assert.notEqual(run({ ORCHD_TASK: "" }).status, 0);
});
