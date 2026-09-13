const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  bumpFor,
  computeBump,
  bumpVersion,
} = require("../scripts/release-bump.mjs");

test("bumpFor classifies conventional-commit subjects", () => {
  assert.equal(bumpFor("feat: add sidebar toggle", true), "minor");
  assert.equal(bumpFor("feat(scope): add thing", true), "minor");
  assert.equal(bumpFor("fix: correct off-by-one", true), "patch");
  assert.equal(bumpFor("fix(scope): correct thing", true), "patch");
  assert.equal(bumpFor("chore: bump deps", true), null);
  assert.equal(bumpFor("Fix a bug (no colon prefix)", true), null);
  assert.equal(bumpFor("random commit message", true), null);
});

test("a breaking marker maps to minor pre-1.0, major post-1.0", () => {
  assert.equal(bumpFor("feat!: drop legacy field", true), "minor");
  assert.equal(bumpFor("fix(scope)!: remove old path", true), "minor");
  assert.equal(bumpFor("feat!: drop legacy field", false), "major");
});

test("computeBump picks the highest-ranked bump across all subjects", () => {
  assert.equal(computeBump(["fix: a", "feat: b", "chore: c"], true), "minor");
  assert.equal(computeBump(["fix: a", "chore: c"], true), "patch");
  assert.equal(computeBump(["chore: c", "docs: d"], true), null);
  assert.equal(computeBump(["fix: a", "feat!: b"], true), "minor");
});

test("bumpVersion bumps the right segment and resets what follows", () => {
  assert.equal(bumpVersion("0.0.6", "patch"), "0.0.7");
  assert.equal(bumpVersion("0.0.6", "minor"), "0.1.0");
  assert.equal(bumpVersion("0.5.3", "major"), "1.0.0");
  assert.equal(bumpVersion("1.2.9", "patch"), "1.2.10");
});
