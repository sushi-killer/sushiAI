const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { manageSkill } = require("../electron/skills-manager.cjs");

async function fixture() {
  return fs.mkdtemp(path.join(os.tmpdir(), "sushiai-skill-manager-"));
}

test("disables and enables Codex skills through skills.config", async () => {
  const home = await fixture();
  const file = path.join(home, ".codex/skills/example/SKILL.md");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, "# Example\n");
  await fs.writeFile(
    path.join(home, ".codex/config.toml"),
    `model = "test"\n\n[[skills.config]]\npath = "${file}"\nenabled = true\n`,
  );
  const item = {
    name: "example",
    path: `~/${path.relative(home, file)}`,
    provider: "Codex",
    source: "Local",
  };
  const executable = () => null;

  await manageSkill({ home, item, action: "disable", executable });
  assert.match(
    await fs.readFile(path.join(home, ".codex/config.toml"), "utf8"),
    /enabled = false/,
  );
  await manageSkill({ home, item, action: "enable", executable });
  assert.match(
    await fs.readFile(path.join(home, ".codex/config.toml"), "utf8"),
    /enabled = true/,
  );
});

test("disables and enables Claude skills through skillOverrides", async () => {
  const home = await fixture();
  const file = path.join(home, ".claude/skills/example/SKILL.md");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, "# Example\n");
  await fs.writeFile(
    path.join(home, ".claude/settings.json"),
    JSON.stringify({ skillOverrides: { other: "name-only" } }),
  );
  const item = {
    name: "example",
    path: `~/${path.relative(home, file)}`,
    provider: "Claude",
    source: "Local",
  };

  await manageSkill({ home, item, action: "disable" });
  let settings = JSON.parse(
    await fs.readFile(path.join(home, ".claude/settings.json"), "utf8"),
  );
  assert.equal(settings.skillOverrides.example, "off");
  await manageSkill({ home, item, action: "enable" });
  settings = JSON.parse(
    await fs.readFile(path.join(home, ".claude/settings.json"), "utf8"),
  );
  assert.equal(settings.skillOverrides.example, undefined);
  assert.equal(settings.skillOverrides.other, "name-only");
});

test("moves a local skill to the system Trash and removes its Codex override", async () => {
  const home = await fixture();
  const file = path.join(home, ".codex/skills/example/SKILL.md");
  const trash = path.join(home, "Trash");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.mkdir(trash);
  await fs.writeFile(file, "# Example\n");
  await fs.writeFile(
    path.join(home, ".codex/config.toml"),
    `[[skills.config]]\npath = "${file}"\nenabled = false\n`,
  );
  const item = {
    name: "example",
    path: `~/${path.relative(home, file)}`,
    provider: "Codex",
    source: "Local",
  };
  const shell = {
    async trashItem(directory) {
      await fs.rename(directory, path.join(trash, path.basename(directory)));
    },
  };

  await manageSkill({ home, item, action: "delete", shell });
  await assert.rejects(fs.access(path.dirname(file)));
  assert.doesNotMatch(
    await fs.readFile(path.join(home, ".codex/config.toml"), "utf8"),
    /skills\.config/,
  );
});
