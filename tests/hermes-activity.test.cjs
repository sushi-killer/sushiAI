const test = require("node:test");
const assert = require("node:assert/strict");
const { activityFor } = require("../electron/agents/hermes-activity.cjs");
const complete = { type: "tool.complete" };
test("only committed memory writes produce activity", () => {
  const item = {
    id: "t1",
    name: "memory",
    status: "complete",
    input: { action: "add" },
    output: { success: true },
  };
  assert.equal(activityFor(complete, item).title, "Memory added");
  assert.equal(
    activityFor(complete, { ...item, output: { success: true, staged: true } }),
    null,
  );
  assert.equal(
    activityFor(complete, { ...item, output: { success: false } }),
    null,
  );
  assert.equal(
    activityFor(complete, { ...item, output: { message: "saved" } }),
    null,
  );
  assert.equal(activityFor({ type: "tool.start" }, item), null);
  assert.equal(
    activityFor(complete, { ...item, input: { action: "read" } }),
    null,
  );
  assert.equal(
    activityFor(complete, {
      ...item,
      input: { operations: [{ action: "add" }, { action: "remove" }] },
    }).summary,
    "2 memory operations completed. Duplicate entries may be unchanged.",
  );
});
test("successful duplicate or unchanged results do not announce new memories or skills", () => {
  for (const name of ["memory", "skill_manage"]) {
    const item = { id: "unchanged", name, status: "complete",
      input: { action: name === "memory" ? "add" : "patch" } };
    for (const marker of [{ changed: false }, { noop: true }, { status: "unchanged" }]) {
      assert.equal(activityFor(complete, { ...item, output: { success: true, ...marker } }), null);
    }
  }
  assert.equal(activityFor(complete, {
    id: "duplicate", name: "memory", status: "complete", input: { action: "add" },
    output: { success: true, message: "Entry already exists (no duplicate added)." },
  }), null);
});
test("skill mutations and native reviews stay distinct from model claims", () => {
  const item = {
    id: "t",
    name: "skill_manage",
    status: "complete",
    input: { action: "create", name: "example" },
    output: { success: true },
  };
  assert.equal(activityFor(complete, item).title, "Skill created");
  assert.equal(
    activityFor(complete, {
      ...item,
      input: { action: "patch", name: "example" },
    }).title,
    "Skill improved",
  );
  assert.equal(
    activityFor({
      type: "message.delta",
      payload: { text: "I improved my skills" },
    }),
    null,
  );
  assert.equal(
    activityFor({
      type: "review.summary",
      payload: { text: "Native review details" },
    }).kind,
    "self-improvement",
  );
});
