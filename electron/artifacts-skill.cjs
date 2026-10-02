const fs = require("node:fs");
const path = require("node:path");
const { ARTIFACTS_MANIFEST } = require("./extensions/builtin-artifacts.cjs");
const {
  registerBuiltinSkill,
  syncLocalBuiltinSkills,
} = require("./extensions/builtin-skills.cjs");

const NAME = "sushiai-artifacts";
const SKILL_PATH = path.join(__dirname, "extensions", "artifacts-skill.md");

/** Registers the Artifacts skill with the builtin-skill hook and, when
 * `subscribe` is given, re-syncs this machine's homes (`home`,
 * `codexAccountsDir`) whenever the extension is turned on or off. SSH hosts
 * follow on their next setup. */
function configureArtifactsSkill({
  isEnabled,
  subscribe,
  home,
  codexAccountsDir,
}) {
  registerBuiltinSkill({
    extensionId: ARTIFACTS_MANIFEST.id,
    name: NAME,
    text: fs.readFileSync(SKILL_PATH, "utf8"),
    isEnabled,
  });
  if (typeof subscribe === "function")
    subscribe((id) => {
      if (id === ARTIFACTS_MANIFEST.id)
        syncLocalBuiltinSkills(home, undefined, codexAccountsDir);
    });
}

module.exports = { configureArtifactsSkill };
