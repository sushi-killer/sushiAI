const { test } = require("node:test");
const assert = require("node:assert/strict");

test("moduleUis is a frozen list of complete ModuleUi entries", async () => {
  const { moduleUis } = await import("../src/extensions/modules.ts");
  assert.ok(Object.isFrozen(moduleUis));
  assert.ok(moduleUis.length > 0);
  for (const ui of moduleUis) {
    assert.equal(typeof ui.extensionId, "string");
    for (const key of ["useAttention", "useWorktreeClaims", "useShell", "act"])
      assert.equal(typeof ui[key], "function", key);
    assert.ok(ui.AttentionDetail);
  }
});

test("activeProjectSections drops the sections of an inactive extension", async () => {
  const { activeProjectSections, projectEnvironmentSections } =
    await import("../src/extensions/modules.ts");
  assert.ok(Object.isFrozen(projectEnvironmentSections));
  assert.ok(projectEnvironmentSections.length > 0);
  const [section] = projectEnvironmentSections;
  const snapshot = (status) => ({
    extensions: [{ manifest: { id: section.extensionId }, status }],
  });
  assert.deepEqual(activeProjectSections([section], snapshot("active")), [
    section,
  ]);
  assert.deepEqual(activeProjectSections([section], snapshot("disabled")), []);
  assert.deepEqual(activeProjectSections([section], { extensions: [] }), []);
});
