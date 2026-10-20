const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const skills = require("../electron/extensions/builtin-skills.cjs");
const {
  VAULT_MANIFEST,
  VAULT_SKILL,
} = require("../electron/extensions/builtin-vault.cjs");

const NAME = VAULT_SKILL.name;
const TEXT = fs.readFileSync(
  path.join(__dirname, "../electron/extensions", VAULT_SKILL.file),
  "utf8",
);

test("the vault skill follows the Vault extension's on and off state", async () => {
  let enabled = true;
  let listener;
  skills.registerExtensionSkill({
    extensionId: VAULT_MANIFEST.id,
    ...VAULT_SKILL,
    isEnabled: () => enabled,
    subscribe: (fn) => (listener = fn),
  });
  const runs = [];
  skills.configureBuiltinSkillSync({
    binary: () => "/bin/sushiai",
    env: {},
    run: async (binary, args, { input }) => runs.push({ args, input }),
  });
  await skills.syncLocalBuiltinSkills();
  enabled = false;
  await listener(VAULT_MANIFEST.id, false);
  assert.equal(listener("builtin.other", true), undefined);
  assert.deepEqual(
    runs.filter((run) => run.args[2] === NAME),
    [
      { args: ["skills", "install", NAME], input: TEXT },
      { args: ["skills", "remove", NAME], input: "" },
    ],
  );
});

test("the vault skill file has a name and a description in its frontmatter", () => {
  assert.match(TEXT, /^---\nname: sushiai-vault\ndescription: .+\n---\n/);
});

test("an SSH host gets the vault skill removed, never installed", async () => {
  skills.registerExtensionSkill({
    extensionId: VAULT_MANIFEST.id,
    ...VAULT_SKILL,
    isEnabled: () => true,
  });
  const calls = [];
  await skills.syncBuiltinSkillsOnHost(
    { exec: async (endpoint, command) => calls.push(command) },
    "ssh:user@devbox",
    {},
  );
  assert.ok(
    calls.includes(`"$HOME/.sushiai/bin/sushiai" skills remove ${NAME}`),
  );
  assert.ok(!calls.some((c) => c.includes(`install ${NAME}`)));
});

test("each built-in skill file exists and is named like its binding", () => {
  const {
    ARTIFACTS_SKILL,
  } = require("../electron/extensions/builtin-artifacts.cjs");
  const {
    ROUTINES_SKILL,
  } = require("../electron/extensions/builtin-routines.cjs");
  for (const skill of [ARTIFACTS_SKILL, ROUTINES_SKILL, VAULT_SKILL]) {
    const text = fs.readFileSync(
      path.join(__dirname, "../electron/extensions", skill.file),
      "utf8",
    );
    assert.match(text, new RegExp(`^---\\nname: ${skill.name}\\n`));
  }
});
