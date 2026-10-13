const { test } = require("node:test");
const assert = require("node:assert/strict");

const model = import("../src/orchestrator/blueprintModel.ts");

const idle = { status: "queued", attempts: [] };

test("a different preset on a task that can take it is written", async () => {
  const { presetToWrite } = await model;
  assert.equal(presetToWrite(idle, "", "careful"), "careful");
  assert.equal(presetToWrite(idle, "fast", "careful"), "careful");
  assert.equal(
    presetToWrite(
      { status: "waiting", attempts: [{ stage: "plan" }] },
      "",
      "fast",
    ),
    "fast",
  );
});

test("an empty or unchanged preset is never written", async () => {
  const { presetToWrite } = await model;
  assert.equal(presetToWrite(idle, "fast", ""), null);
  assert.equal(presetToWrite(idle, "fast", "fast"), null);
});

test("a running or drafting task never gets a preset write", async () => {
  const { presetToWrite } = await model;
  assert.equal(
    presetToWrite({ status: "running", attempts: [] }, "", "careful"),
    null,
  );
  assert.equal(
    presetToWrite({ status: "drafting", attempts: [] }, "", "careful"),
    null,
  );
});

test("a task with an implement attempt never gets a preset write", async () => {
  const { presetToWrite } = await model;
  assert.equal(
    presetToWrite(
      { status: "waiting", attempts: [{ stage: "implement" }] },
      "",
      "careful",
    ),
    null,
  );
});
