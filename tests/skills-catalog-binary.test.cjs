// The Skills catalog and the builtin skills through the real sushiai binary
// (`cargo build -p sushiai` first): only Claude Code and Codex are listed, and
// the app writes no skill folder itself.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { execFileSync } = require("node:child_process");
const { scanLocalSkills } = require("../electron/skills-catalog.cjs");
const {
  configureBuiltinSkillSync,
  registerBuiltinSkill,
  syncLocalBuiltinSkills,
} = require("../electron/extensions/builtin-skills.cjs");

const BINARY = path.join(__dirname, "..", "target", "debug", "sushiai");

// Folders of other harnesses: never in the catalog.
const OTHER_FOLDERS = [
  ".gemini/antigravity/skills",
  ".cursor/skills",
  ".cursor/skills-cursor",
  ".windsurf/skills",
  ".opencode/skills",
  ".goose/skills",
  ".amp/skills",
  ".factory/skills",
  ".roo/skills",
  ".continue/skills",
  ".local/share/terminal-browser/app/skills",
];

function binary() {
  assert.ok(
    fs.existsSync(BINARY),
    `${BINARY} is missing: run cargo build -p sushiai`,
  );
  return BINARY;
}

function tempHome(t) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "skills-binary-"));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  return home;
}

function writeSkill(home, relativeDir, name) {
  const dir = path.join(home, relativeDir);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "SKILL.md"),
    `---\nname: ${name}\ndescription: ${name} skill\n---\n\n# ${name}\n`,
  );
}

/** The binary's environment: this home, none of this machine's tool folders. */
const envFor = (home) => ({ HOME: home, PATH: process.env.PATH });

test("the CLI and the app's catalog list only Claude Code and Codex rows", async (t) => {
  const home = tempHome(t);
  writeSkill(home, ".claude/skills/claude-one", "claude-one");
  writeSkill(home, ".codex/skills/codex-one", "codex-one");
  writeSkill(home, ".agents/skills/agent-one", "agent-one");
  writeSkill(home, ".claude/skills/catalog/nested-one", "nested-one");
  OTHER_FOLDERS.forEach((folder, i) =>
    writeSkill(home, `${folder}/other-${i}`, `other-${i}`),
  );

  const cli = JSON.parse(
    execFileSync(binary(), ["skills", "--json", "--catalog"], {
      env: envFor(home),
      encoding: "utf8",
    }),
  );
  const items = await scanLocalSkills({
    binary: binary(),
    home,
    env: envFor(home),
  });
  for (const rows of [cli, items]) {
    assert.deepEqual(
      rows.map((row) => [row.name, row.harness, row.provider]),
      [
        ["agent-one", "Codex", "Codex"],
        ["claude-one", "Claude Code", "Claude"],
        ["codex-one", "Codex", "Codex"],
      ],
    );
    assert.ok(rows.every((row) => !row.isDuplicate));
  }
  assert.ok(items.every((item) => item.path.startsWith("~/")));
});

test("the builtin skills go into Claude Code, Codex and each account home, and leave when off", async (t) => {
  const home = tempHome(t);
  fs.mkdirSync(path.join(home, ".claude"));
  fs.mkdirSync(path.join(home, ".codex"));
  const accounts = path.join(home, "codex-accounts");
  const account = path.join(accounts, "a1");
  fs.mkdirSync(account, { recursive: true });
  let enabled = true;
  const name = "test-binary-skill";
  const text = `---\nname: ${name}\ndescription: x\n---\n`;
  registerBuiltinSkill({
    extensionId: "builtin.test",
    name,
    text,
    isEnabled: () => enabled,
  });
  configureBuiltinSkillSync({
    binary,
    codexAccountsDir: accounts,
    env: envFor(home),
  });
  const files = [
    path.join(home, ".claude/skills", name, "SKILL.md"),
    path.join(home, ".codex/skills", name, "SKILL.md"),
    path.join(account, "skills", name, "SKILL.md"),
  ];
  await syncLocalBuiltinSkills();
  for (const file of files) assert.equal(fs.readFileSync(file, "utf8"), text);
  assert.equal(fs.existsSync(path.join(home, ".agents")), false);
  assert.deepEqual(await syncLocalBuiltinSkills(), []);

  enabled = false;
  await syncLocalBuiltinSkills();
  for (const file of files)
    assert.equal(fs.existsSync(path.dirname(file)), false, file);
});
