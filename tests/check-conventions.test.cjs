const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { addsMarkdownFragment } = require("../scripts/lib/release-notes.mjs");

function run(env) {
  return spawnSync(process.execPath, ["scripts/check-conventions.mjs"], {
    encoding: "utf8",
    env: { ...process.env, ...env },
  });
}

test("feature/ branch paired with a feat: title passes", () => {
  const result = run({
    HEAD_BRANCH: "feature/task-list",
    PR_TITLE: "feat: add task list",
  });
  assert.equal(result.status, 0);
});

test("fix/ branch rejects a feat: title", () => {
  const result = run({ HEAD_BRANCH: "fix/leak", PR_TITLE: "feat: not a fix" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /needs a "fix: " PR title/);
});

test("an unrecognized branch prefix is rejected", () => {
  const result = run({
    HEAD_BRANCH: "my-random-branch",
    PR_TITLE: "feat: whatever",
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /doesn't match \(feature\|fix\|chore\|release\)/);
});

test("chore/ accepts any conventional-commit type", () => {
  const result = run({
    HEAD_BRANCH: "chore/deps",
    PR_TITLE: "docs: update readme",
  });
  assert.equal(result.status, 0);
});

test("release/ requires a chore(release): title", () => {
  const good = run({
    HEAD_BRANCH: "release/0.0.7",
    PR_TITLE: "chore(release): v0.0.7",
  });
  assert.equal(good.status, 0);
  const bad = run({ HEAD_BRANCH: "release/0.0.7", PR_TITLE: "chore: v0.0.7" });
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /needs a "chore\(release\): " PR title/);
});

test("a title with no conventional-commit prefix is rejected", () => {
  const result = run({ HEAD_BRANCH: "feature/x", PR_TITLE: "add task list" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /doesn't start with a conventional-commit type/);
});

// The breaking-change-needs-a-fragment rule's git plumbing (spawning `git
// diff` between BASE_REF/HEAD_REF) isn't unit tested here - it needs real
// repo history depth that a shallow CI checkout doesn't have. This tests
// the pure predicate instead, which is the actual logic under test; the
// real `conventions` CI job exercises the plumbing with fetch-depth: 0.
test("addsMarkdownFragment finds an added .md file among other added files", () => {
  assert.equal(
    addsMarkdownFragment("A\tdocs/releases/unreleased/.gitkeep\n"),
    false,
  );
  assert.equal(
    addsMarkdownFragment(
      "A\tdocs/releases/unreleased/.gitkeep\nA\tdocs/releases/unreleased/my-change.md\n",
    ),
    true,
  );
  assert.equal(addsMarkdownFragment(""), false);
});

test("docs/LESSONS.md within its caps passes, fenced example not counted", () => {
  const result = run({
    LESSONS_PATH_OVERRIDE: "tests/fixtures/lessons/clean.md",
  });
  assert.equal(result.status, 0);
});

test("docs/LESSONS.md over the open-entry-count cap fails", () => {
  const result = run({
    LESSONS_PATH_OVERRIDE: "tests/fixtures/lessons/too-many-open.md",
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /9 open entries \(cap: 8\)/);
});

test("docs/LESSONS.md with an over-budget entry fails", () => {
  const result = run({
    LESSONS_PATH_OVERRIDE: "tests/fixtures/lessons/entry-too-long.md",
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /open entry #1 is \d+ words \(cap: 120\)/);
});
