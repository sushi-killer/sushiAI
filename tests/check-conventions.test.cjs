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

test("a private network address in source fails, a documentation example does not", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  // Built at runtime on purpose: this file lives in tests/, which the scan
  // walks, so a literal dotted private address here would fail CI on itself.
  const privateHost = ["172", "20", "0", "5"].join(".");
  const leak = fs.mkdtempSync(path.join(os.tmpdir(), "ip-leak-"));
  fs.mkdirSync(path.join(leak, "tests"));
  fs.writeFileSync(
    path.join(leak, "tests", "probe.test.cjs"),
    `const host = "${privateHost}";\n`,
  );
  const failed = run({ PRIVATE_IP_ROOT_OVERRIDE: leak });
  assert.equal(failed.status, 1);
  assert.match(
    failed.stderr,
    /tests\/probe\.test\.cjs:1 contains a private network address/,
  );

  // RFC 5737 documentation space is the sanctioned way to write an example.
  const clean = fs.mkdtempSync(path.join(os.tmpdir(), "ip-clean-"));
  fs.mkdirSync(path.join(clean, "src"));
  fs.writeFileSync(
    path.join(clean, "src", "example.ts"),
    `const host = "${["192", "0", "2", "10"].join(".")}";\n`,
  );
  const passed = run({ PRIVATE_IP_ROOT_OVERRIDE: clean });
  assert.equal(passed.status, 0, passed.stderr);

  fs.rmSync(leak, { recursive: true, force: true });
  fs.rmSync(clean, { recursive: true, force: true });
});

test("skills and subagents with a broken shape fail, a clean setup passes", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const write = (dir, file, text) => {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), text);
  };
  const skill = (dir, name, head) => {
    write(dir, `.agents/skills/${name}/SKILL.md`, `---\n${head}\n---\n`);
  };
  const link = (dir, name) => {
    fs.mkdirSync(path.join(dir, ".claude/skills"), { recursive: true });
    fs.symlinkSync(
      `../../.agents/skills/${name}`,
      path.join(dir, ".claude/skills", name),
    );
  };

  const broken = fs.mkdtempSync(path.join(os.tmpdir(), "agents-broken-"));
  skill(broken, "good", "name: good\ndescription: Use when testing.");
  link(broken, "good");
  skill(broken, "bad", "name: wrong");
  write(
    broken,
    ".claude/agents/role.md",
    "---\nname: role\ndescription: A role.\nmodel: inherit\nskills: [missing]\n---\nFollow `$nowhere`.\n",
  );
  const failed = run({ AGENT_SETUP_ROOT_OVERRIDE: broken });
  assert.equal(failed.status, 1);
  assert.match(failed.stderr, /bad\/SKILL\.md: name "wrong" must match/);
  assert.match(failed.stderr, /bad\/SKILL\.md: no description/);
  assert.match(failed.stderr, /\.claude\/skills\/bad must be a symlink/);
  assert.match(failed.stderr, /role\.md: model "inherit"/);
  assert.match(failed.stderr, /preloads unknown skill "missing"/);
  assert.match(failed.stderr, /`\$nowhere` is not a skill/);

  const clean = fs.mkdtempSync(path.join(os.tmpdir(), "agents-clean-"));
  skill(clean, "good", "name: good\ndescription: Use when testing.");
  link(clean, "good");
  write(
    clean,
    ".claude/agents/role.md",
    "---\nname: role\ndescription: A role.\nmodel: sonnet\nskills: [good]\n---\nFollow `$good`.\n",
  );
  const passed = run({ AGENT_SETUP_ROOT_OVERRIDE: clean });
  assert.equal(passed.status, 0, passed.stderr);

  fs.rmSync(broken, { recursive: true, force: true });
  fs.rmSync(clean, { recursive: true, force: true });
});
