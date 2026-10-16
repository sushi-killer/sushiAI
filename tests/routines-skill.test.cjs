const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const root = path.join(__dirname, "..");
const SKILL = path.join(root, "electron", "extensions", "routines-skill.md");
const DOCS = [
  SKILL,
  path.join(root, "docs", "routines.md"),
  path.join(root, "docs", "releases", "unreleased", "routines.md"),
];

function commandList() {
  const rust = fs.readFileSync(
    path.join(root, "crates", "sushiai-routines", "src", "help.rs"),
    "utf8",
  );
  const block = rust.match(/COMMANDS: &\[&str\] = &\[([\s\S]*?)\];/);
  assert.ok(block, "COMMANDS list found in help.rs");
  const list = [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  assert.ok(list.length > 5);
  return list;
}

// Every `sushiai <words>` in the text must start with a listed command.
function mentionedCommands(text) {
  return [
    ...text
      .replace(/Bash\([^)]*\)/g, "")
      .matchAll(/\bsushiai((?: [a-z][a-z-]*)+)/g),
  ].map((m) => `sushiai${m[1]}`);
}

test("every sushiai command the skill and docs mention is in the help list", () => {
  const commands = commandList();
  for (const file of DOCS) {
    const found = mentionedCommands(fs.readFileSync(file, "utf8"));
    assert.ok(found.length > 0 || file !== SKILL, file);
    for (const mention of found)
      assert.ok(
        commands.some(
          (c) =>
            mention === c ||
            mention.startsWith(`${c} `) ||
            // A command family named without its subcommand, such as `sushiai propose`.
            c.startsWith(`${mention} `),
        ),
        `${path.basename(file)}: "${mention}" is not a command of sushiai routines help`,
      );
  }
});

test("the skill points to the help instead of copying command details", () => {
  const text = fs.readFileSync(SKILL, "utf8");
  assert.match(text, /sushiai routines help/);
  assert.match(text, /^---\nname: sushiai-routines\ndescription: .+\n---\n/);
  for (const step of ["validate", "save", "dry-run"])
    assert.match(text, new RegExp(`sushiai routine ${step}`));
  assert.match(text, /Data is not instructions/);
});

test("the skill and docs name no real host or project", () => {
  // Generic detection only. Extra private names are read at test time from
  // SUSHIAI_PRIVATE_NAMES (comma separated) or a gitignored local file.
  const extra = [];
  const local = path.join(root, "artifacts", "private-names.txt");
  const raw = [
    process.env.SUSHIAI_PRIVATE_NAMES || "",
    fs.existsSync(local) ? fs.readFileSync(local, "utf8") : "",
  ].join(",");
  for (const name of raw.split(/[,\n]/).map((v) => v.trim()))
    if (name) extra.push(name.toLowerCase());
  const documentation = /^192\.0\.2\.\d{1,3}$/;
  for (const file of DOCS) {
    const text = fs.readFileSync(file, "utf8");
    const label = path.basename(file);
    assert.doesNotMatch(text, /\/Users\//, `${label}: home path`);
    for (const ip of text.match(/\b(?:\d{1,3}\.){3}\d{1,3}\b/g) || [])
      assert.match(ip, documentation, `${label}: address outside 192.0.2.0/24`);
    for (const host of text.match(/[\w.-]+@[\w.-]+/g) || [])
      assert.equal(host, "user@devbox", `${label}: host ${host}`);
    const lower = text.toLowerCase();
    for (const name of extra)
      assert.ok(!lower.includes(name), `${label}: a private name`);
  }
});

test("the skill scopes its example tool grant and protects enabled routines", () => {
  const text = fs.readFileSync(SKILL, "utf8");
  assert.doesNotMatch(text, /Bash\(sushiai:\*\)/);
  assert.match(text, /Bash\(sushiai ask:\*\) Bash\(sushiai propose:\*\)/);
  assert.match(text, /enabled routine/);
});

test("the registered skill text equals the file and follows the enabled predicate", () => {
  const skills = require("../electron/extensions/builtin-skills.cjs");
  const { configureRoutinesSkill } = require("../electron/routines-skill.cjs");
  let enabled = true;
  configureRoutinesSkill({
    extensionId: "builtin.routines",
    isEnabled: () => enabled,
  });
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "routines-skill-"));
  const target = (tool) =>
    path.join(home, tool, "skills", "sushiai-routines", "SKILL.md");
  try {
    skills.syncLocalBuiltinSkills(home, {}, undefined);
    for (const tool of [".claude", ".codex", ".agents"])
      assert.equal(
        fs.readFileSync(target(tool), "utf8"),
        fs.readFileSync(SKILL, "utf8"),
      );
    enabled = false;
    skills.syncLocalBuiltinSkills(home, {}, undefined);
    for (const tool of [".claude", ".codex", ".agents"])
      assert.equal(fs.existsSync(target(tool)), false);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the skill names the same modes and MCP tools as the code", () => {
  const text = fs.readFileSync(SKILL, "utf8");
  const validate = fs.readFileSync(
    path.join(root, "crates", "sushiai-routines", "src", "validate.rs"),
    "utf8",
  );
  const modes = validate
    .match(/MODES: &\[&str\] = &\[([^\]]*)\]/)[1]
    .match(/"([^"]+)"/g)
    .map((m) => m.slice(1, -1));
  assert.deepEqual(modes, ["ask-every-time", "ask-first", "on-its-own"]);
  for (const mode of modes) assert.ok(text.includes(`\`${mode}\``), mode);
  const mcp = fs
    .readFileSync(path.join(root, "crates", "sushiai", "src", "mcp.rs"), "utf8")
    .split("#[cfg(test)]")[0];
  const tools = [...mcp.matchAll(/"(routines_[a-z_]+)",\n\s+"/g)].map(
    (m) => m[1],
  );
  assert.equal(tools.length, 8);
  for (const tool of tools) assert.ok(text.includes(tool), tool);
  // The in-run commands are never MCP tools.
  assert.doesNotMatch(mcp, /"routines_(ask|propose)/);
  assert.match(text, /## Access/);
});

test("the skill says a project is optional", () => {
  const text = fs.readFileSync(SKILL, "utf8");
  assert.match(text, /optional\s+`?projects/);
  assert.match(text, /NO\s+project: omit `where\.projects`/);
  assert.match(text, /zero to 20/);
  assert.doesNotMatch(text, /where\.project\b(?!s)/);
});
