const { test } = require("node:test");
const assert = require("node:assert/strict");

test("moduleUis is a frozen list of complete ModuleUi entries", async () => {
  const { moduleUis } = await import("../src/extensions/modules.ts");
  assert.ok(Object.isFrozen(moduleUis));
  assert.ok(moduleUis.length > 0);
  for (const ui of moduleUis) {
    assert.equal(typeof ui.extensionId, "string");
    for (const key of ["useAttention", "useWorktreeClaims", "useShell"])
      assert.equal(typeof ui[key], "function", key);
    assert.ok(ui.AttentionDetail);
  }
});
