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

test("a userData JSON store named in electron/ is rejected", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "state-store-"));
  try {
    fs.mkdirSync(path.join(dir, "electron"));
    fs.writeFileSync(
      path.join(dir, "electron/store.cjs"),
      'const file = path.join(userData, "settings.json");\n',
    );
    const bad = run({ STATE_STORE_ROOT_OVERRIDE: dir });
    assert.equal(bad.status, 1);
    assert.match(bad.stderr, /electron\/store\.cjs:1 names settings\.json/);
    assert.match(
      bad.stderr,
      /app state goes in sushiai\.db \(electron\/app-db\.cjs\)/,
    );
    fs.writeFileSync(path.join(dir, "electron/store.cjs"), "const n = 1;\n");
    assert.equal(run({ STATE_STORE_ROOT_OVERRIDE: dir }).status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("template and concatenated .json names in electron/ are rejected, comments are not", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "state-store-"));
  try {
    fs.mkdirSync(path.join(dir, "electron"));
    const write = (text) =>
      fs.writeFileSync(path.join(dir, "electron/store.cjs"), text);
    write("const file = `${userData}/state.json`;\n");
    assert.equal(run({ STATE_STORE_ROOT_OVERRIDE: dir }).status, 1);
    write('const file = base + ".json";\n');
    assert.equal(run({ STATE_STORE_ROOT_OVERRIDE: dir }).status, 1);
    write("// a note about `x.json` in a comment\n");
    assert.equal(run({ STATE_STORE_ROOT_OVERRIDE: dir }).status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the retired session backend's name fails the check in any shipped tree", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "retired-name-"));
  try {
    fs.mkdirSync(path.join(root, "electron"));
    fs.mkdirSync(path.join(root, ".agents"));
    // Built at runtime: this file is itself scanned.
    const name = ["Her", "dr"].join("");
    fs.writeFileSync(path.join(root, "electron", "clean.cjs"), "// fine\n");
    assert.equal(
      run({
        HEAD_BRANCH: "chore/x",
        PR_TITLE: "chore: x",
        RETIRED_NAME_ROOT_OVERRIDE: root,
      }).status,
      0,
    );
    fs.writeFileSync(path.join(root, ".agents", "note.md"), `uses ${name}\n`);
    const result = run({
      HEAD_BRANCH: "chore/x",
      PR_TITLE: "chore: x",
      RETIRED_NAME_ROOT_OVERRIDE: root,
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /\.agents\/note\.md:1 names the retired/);
    // promo/, crates/, README.md and AGENTS.md are scanned too.
    fs.rmSync(path.join(root, ".agents", "note.md"));
    fs.mkdirSync(path.join(root, "crates", "x"), { recursive: true });
    fs.mkdirSync(path.join(root, "promo"));
    for (const file of [
      "crates/x/lib.rs",
      "promo/mock.cjs",
      "README.md",
      "AGENTS.md",
    ]) {
      fs.writeFileSync(path.join(root, file), `// ${name}\n`);
      const hit = run({
        HEAD_BRANCH: "chore/x",
        PR_TITLE: "chore: x",
        RETIRED_NAME_ROOT_OVERRIDE: root,
      });
      assert.equal(hit.status, 1, file);
      assert.ok(hit.stderr.includes(`${file}:1 names the retired`), file);
      fs.writeFileSync(path.join(root, file), "// fine\n");
    }
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a script that sets SUSHIAI_HOME without HOME is rejected", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "home-rule-"));
  try {
    fs.mkdirSync(path.join(root, "scripts"));
    const vars = {
      HEAD_BRANCH: "chore/x",
      PR_TITLE: "chore: x",
      HOME_RULE_ROOT_OVERRIDE: root,
    };
    const file = path.join(root, "scripts", "launch.mjs");
    fs.writeFileSync(file, 'const e = { SUSHIAI_HOME: "/tmp/x/sushiai" };\n');
    const bad = run(vars);
    assert.equal(bad.status, 1);
    assert.match(
      bad.stderr,
      /scripts\/launch\.mjs: sets SUSHIAI_HOME without HOME/,
    );
    fs.writeFileSync(
      file,
      'const e = { SUSHIAI_HOME: "/tmp/x/sushiai", HOME: "/tmp/x" };\n',
    );
    assert.equal(run(vars).status, 0);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a core main-process file that requires a module main.cjs loads fails the check", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "core-isolation-"));
  try {
    fs.mkdirSync(path.join(dir, "electron/ipc"), { recursive: true });
    const write = (name, text) =>
      fs.writeFileSync(path.join(dir, "electron", name), text);
    write("main.cjs", 'require("./plugin.cjs");\nrequire("./ipc/router.cjs");\n');
    write("plugin.cjs", "module.exports = {};\n");
    write("ipc/router.cjs", 'require("../plugin.cjs");\n');
    const bad = run({ CORE_ISOLATION_ROOT_OVERRIDE: dir });
    assert.equal(bad.status, 1);
    assert.match(
      bad.stderr,
      /electron\/ipc\/router\.cjs requires electron\/plugin\.cjs, a module main\.cjs plugs in/,
    );
    write("ipc/router.cjs", 'require("node:path");\nrequire("./peer.cjs");\n');
    write("ipc/peer.cjs", "module.exports = {};\n");
    assert.equal(run({ CORE_ISOLATION_ROOT_OVERRIDE: dir }).status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("core may not import a directory the composition root loads as a module", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const tree = fs.mkdtempSync(path.join(os.tmpdir(), "core-module-"));
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(tree, file)), { recursive: true });
    fs.writeFileSync(path.join(tree, file), text);
  };
  write(
    "src/extensions/modules.ts",
    'import { widget } from "../gadget/module.ts";\nexport const modules = [widget];\n',
  );
  write("src/extensions/coreViews.ts", "export const views = [];\n");
  write("src/gadget/module.ts", "export const widget = {};\n");
  write("src/gadget/Panel.tsx", "export const Panel = null;\n");
  write("src/ui/Tag.tsx", "export const Tag = null;\n");
  write("src/app/Fine.tsx", 'import { Tag } from "../ui/Tag.tsx";\n');
  const clean = run({ MODULE_ROOT_OVERRIDE: tree });
  assert.equal(clean.status, 0, clean.stderr);

  write("src/app/Leak.tsx", 'import { Panel } from "../gadget/Panel.tsx";\n');
  const failed = run({ MODULE_ROOT_OVERRIDE: tree });
  assert.equal(failed.status, 1);
  assert.match(
    failed.stderr,
    /src\/app\/Leak\.tsx imports \.\.\/gadget\/Panel\.tsx; core may not import src\/gadget/,
  );

  fs.rmSync(tree, { recursive: true, force: true });
});
