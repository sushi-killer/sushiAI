const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { scanLocalSkills } = require("../electron/skills-catalog.cjs");

async function fixture() {
  return fs.mkdtemp(path.join(os.tmpdir(), "sushiai-skills-"));
}

async function writeSkill(home, relativePath, name, description) {
  const file = path.join(home, relativePath, "SKILL.md");
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(
    file,
    `---\nname: ${name}\ndescription: ${description}\n---\n\n# ${name}\n`,
  );
  return file;
}

test("scans local and plugin skill roots with provider metadata", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  await writeSkill(home, ".codex/skills/codex-one", "codex-one", "Codex skill");
  await writeSkill(
    home,
    ".claude/skills/claude-one",
    "claude-one",
    "Claude skill",
  );
  await writeSkill(
    home,
    ".agents/skills/agent-one",
    "agent-one",
    "Agent skill",
  );
  await writeSkill(
    home,
    ".codex/plugins/cache/example/plugin-one/1.0/skills/plugin-one",
    "plugin-one",
    "Plugin skill",
  );
  await fs.writeFile(
    path.join(home, ".codex/config.toml"),
    `[marketplaces.example]\nsource_type = "local"\nsource = "/missing"\n\n[plugins."plugin-one@example"]\nenabled = true\n`,
  );
  await writeSkill(
    home,
    ".gemini/antigravity/skills/other-one",
    "other-one",
    "Other skill",
  );
  await writeSkill(
    home,
    ".claude/skills/catalog/nested-one",
    "nested-one",
    "Nested catalog entry",
  );

  const items = await scanLocalSkills({ home, snapshotFile, now: Date.now() });
  assert.deepEqual(
    items.map((item) => [item.name, item.provider, item.source]),
    [
      ["agent-one", "Agent", "Local"],
      ["claude-one", "Claude", "Local"],
      ["codex-one", "Codex", "Local"],
      ["other-one", "Other", "External"],
      ["plugin-one", "Codex", "Plugin"],
    ],
  );
  assert.ok(items.every((item) => item.path.startsWith("~/")));
  assert.equal(
    items.some((item) => item.name === "nested-one"),
    false,
  );
  assert.equal(
    items.find((item) => item.name === "plugin-one").availability,
    "active",
  );
});

test("uses Claude usage events and marks stale, unused, duplicate and changed skills", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  const now = Date.now();
  const old = now - 200 * 24 * 60 * 60 * 1000;
  const claudeFile = await writeSkill(
    home,
    ".claude/skills/shared",
    "shared",
    "Claude copy",
  );
  await fs.utimes(claudeFile, new Date(old), new Date(old));
  await writeSkill(home, ".agents/skills/shared", "shared", "Agent copy");
  const unusedFile = await writeSkill(
    home,
    ".codex/skills/unused",
    "unused",
    "Old skill",
  );
  await fs.utimes(unusedFile, new Date(old), new Date(old));
  await fs.mkdir(path.join(home, ".claude"), { recursive: true });
  await fs.writeFile(
    path.join(home, ".claude/skill-events.jsonl"),
    JSON.stringify({
      ts: Math.floor((now - 2 * 60 * 60 * 1000) / 1000),
      skill: "shared",
    }) + "\n",
  );
  await fs.mkdir(path.join(home, ".claude/coach"), { recursive: true });
  await fs.writeFile(
    path.join(home, ".claude/coach/skill-usage-cache.json"),
    JSON.stringify({
      usage: {
        shared: {
          calls: 4,
          lastUsed: (now - 3 * 60 * 60 * 1000) / 1000,
        },
      },
    }),
  );

  const first = await scanLocalSkills({ home, snapshotFile, now });
  const shared = first.filter((item) => item.name === "shared");
  assert.equal(shared.length, 2);
  assert.ok(shared.every((item) => item.isDuplicate));
  assert.equal(
    shared.find((item) => item.provider === "Claude").lastUsedSource,
    "skill event",
  );
  assert.equal(
    shared.find((item) => item.provider === "Agent").lastUsedSource,
    "filesystem access",
  );
  assert.equal(
    shared.find((item) => item.provider === "Claude").isRecent,
    true,
  );
  assert.equal(shared.find((item) => item.provider === "Claude").usageCount, 4);
  assert.equal(
    shared.find((item) => item.provider === "Claude").recentUses.length,
    1,
  );
  assert.ok(first.find((item) => item.name === "unused").isUnused);
  assert.ok(first.find((item) => item.name === "unused").isStale);

  await fs.writeFile(
    claudeFile,
    "---\nname: shared\ndescription: Updated Claude copy\n---\n\n# changed\n",
  );
  const second = await scanLocalSkills({ home, snapshotFile, now });
  const changed = second.find(
    (item) => item.path === "~/.claude/skills/shared/SKILL.md",
  );
  assert.equal(changed.changed, true);
  assert.equal(changed.needsReview, true);
});

test("follows top-level skill symlinks without counting nested catalog entries", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  const target = await writeSkill(
    home,
    ".claude/skill-sources/real-skill",
    "real-skill",
    "Symlinked skill",
  );
  await fs.mkdir(path.join(home, ".claude/skills"), { recursive: true });
  await fs.symlink(
    path.dirname(target),
    path.join(home, ".claude/skills/real-skill"),
    "dir",
  );
  await writeSkill(
    home,
    ".claude/skills/catalog/nested-skill",
    "nested-skill",
    "Should not be a root skill",
  );

  const items = await scanLocalSkills({ home, snapshotFile, now: Date.now() });
  assert.deepEqual(
    items.map((item) => item.name),
    ["real-skill"],
  );
  assert.equal(items[0].path, "~/.claude/skills/real-skill/SKILL.md");
});

test("reads Codex skill and plugin disabled state from the provider config", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  const localFile = await writeSkill(
    home,
    ".codex/skills/disabled-local",
    "disabled-local",
    "Disabled local skill",
  );
  await writeSkill(
    home,
    ".codex/plugins/cache/example/plugin-one/1.0/skills/disabled-plugin",
    "disabled-plugin",
    "Disabled plugin skill",
  );
  await fs.writeFile(
    path.join(home, ".codex/config.toml"),
    `[marketplaces.example]\nsource = "/missing"\n\n[[skills.config]]\npath = "${localFile}"\nenabled = false\n\n[plugins."plugin-one@example"]\nenabled = false\n`,
  );

  const items = await scanLocalSkills({ home, snapshotFile, now: Date.now() });
  assert.equal(
    items.find((item) => item.name === "disabled-local").availability,
    "disabled",
  );
  assert.equal(
    items.find((item) => item.name === "disabled-plugin").availability,
    "disabled",
  );
});

test("does not apply Claude skillOverrides to plugin skills", async () => {
  const home = await fixture();
  const snapshotFile = path.join(home, "app", "skills.json");
  await writeSkill(
    home,
    ".claude/plugins/cache/example/plugin-one/1.0/skills/shared",
    "shared",
    "Plugin skill",
  );
  await fs.mkdir(path.join(home, ".claude/plugins"), { recursive: true });
  await fs.writeFile(
    path.join(home, ".claude/plugins/installed_plugins.json"),
    JSON.stringify({
      version: 2,
      plugins: {
        "plugin-one@example": [
          {
            scope: "user",
            installPath: path.join(
              home,
              ".claude/plugins/cache/example/plugin-one/1.0",
            ),
          },
        ],
      },
    }),
  );
  await fs.writeFile(
    path.join(home, ".claude/settings.json"),
    JSON.stringify({
      enabledPlugins: { "plugin-one@example": true },
      skillOverrides: { shared: "off" },
    }),
  );

  const items = await scanLocalSkills({ home, snapshotFile, now: Date.now() });
  assert.equal(items[0].source, "Plugin");
  assert.equal(items[0].availability, "active");
});
