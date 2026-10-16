const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { manageSkill } = require("../electron/skills-manager.cjs");

async function fixture() {
  return fs.mkdtemp(path.join(os.tmpdir(), "sushiai-skill-manager-"));
}

/** The main process's catalog: the row for `item`, with the root its scan
 * found it under. */
const catalogFor = (item, root) => [{ ...item, root }];

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
  const catalog = catalogFor(item, path.join(home, ".codex/skills"));

  await manageSkill({ home, item, catalog, action: "disable", executable });
  assert.match(
    await fs.readFile(path.join(home, ".codex/config.toml"), "utf8"),
    /enabled = false/,
  );
  await manageSkill({ home, item, catalog, action: "enable", executable });
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
  const catalog = catalogFor(item, path.join(home, ".claude/skills"));

  await manageSkill({ home, item, catalog, action: "disable" });
  let settings = JSON.parse(
    await fs.readFile(path.join(home, ".claude/settings.json"), "utf8"),
  );
  assert.equal(settings.skillOverrides.example, "off");
  await manageSkill({ home, item, catalog, action: "enable" });
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

  await manageSkill({
    home,
    item,
    catalog: catalogFor(item, path.join(home, ".codex/skills")),
    action: "delete",
    shell,
  });
  await assert.rejects(fs.access(path.dirname(file)));
  assert.doesNotMatch(
    await fs.readFile(path.join(home, ".codex/config.toml"), "utf8"),
    /skills\.config/,
  );
});

test("a change uses the catalog row's root, never one the renderer sends", async () => {
  const home = await fixture();
  const file = path.join(home, "elsewhere/victim/SKILL.md");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, "# Victim\n");
  const trashed = [];
  const shell = { trashItem: async (directory) => trashed.push(directory) };
  const forged = {
    name: "victim",
    path: `~/${path.relative(home, file)}`,
    provider: "Claude",
    source: "Local",
    root: path.join(home, "elsewhere"),
  };
  // Not in the catalog at all.
  await assert.rejects(
    manageSkill({ home, item: forged, catalog: [], action: "delete", shell }),
    /not in the current catalog/,
  );
  // In the catalog, under the root the scan found: the renderer's root is ignored.
  await assert.rejects(
    manageSkill({
      home,
      item: forged,
      catalog: [{ ...forged, root: path.join(home, ".claude/skills") }],
      action: "delete",
      shell,
    }),
    /managed local roots/,
  );
  assert.deepEqual(trashed, []);
  await fs.access(file);
});

test("a row is only the one the catalog names: another provider is not it", async () => {
  const home = await fixture();
  const file = path.join(home, ".codex/skills/example/SKILL.md");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, "# Example\n");
  const row = {
    name: "example",
    path: `~/${path.relative(home, file)}`,
    provider: "Codex",
    source: "Local",
    root: path.join(home, ".codex/skills"),
  };
  await assert.rejects(
    manageSkill({
      home,
      item: { ...row, provider: "Claude" },
      catalog: [row],
      action: "disable",
    }),
    /not in the current catalog/,
  );
});
