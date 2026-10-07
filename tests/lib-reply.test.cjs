const { test } = require("node:test");
const assert = require("node:assert/strict");
const load = () => import("../src/lib/reply.ts");

test("typed text replaces a pick; a pick alone is sent as is", async () => {
  const {
    composeReply,
    shownPick,
    clickReply,
    enterReply,
    togglePick,
    ENTER_ARM_MS,
  } = await load();
  assert.equal(composeReply("a", " typed "), "typed");
  assert.equal(composeReply("a", "  "), "a");
  const fresh = { pick: undefined, preselected: "first", note: "" };
  assert.equal(shownPick(fresh), "first");
  assert.equal(clickReply(fresh), "first");
  assert.equal(enterReply(fresh, 0, ENTER_ARM_MS), "");
  assert.equal(enterReply({ ...fresh, pick: "b" }, 0, ENTER_ARM_MS), "b");
  assert.equal(enterReply({ ...fresh, pick: "b" }, 0, ENTER_ARM_MS - 1), "");
  assert.equal(shownPick({ ...fresh, note: "x" }), "");
  assert.equal(togglePick(fresh, "first"), "");
  assert.equal(togglePick(fresh, "second"), "second");
});
