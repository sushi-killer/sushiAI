const fs = require("node:fs");
const path = require("node:path");
const {
  registerBuiltinSkill,
  syncLocalBuiltinSkills,
} = require("./extensions/builtin-skills.cjs");

const NAME = "sushiai-routines";
const SKILL_PATH = path.join(__dirname, "extensions", "routines-skill.md");

/** Registers the Routines builder skill with the builtin-skill hook. The skill
 * is removed from every home while `isEnabled()` is false. `subscribe` (the
 * extension manager's onChange) re-syncs this machine when the extension
 * `extensionId` is turned on or off. */
function configureRoutinesSkill({
  extensionId,
  isEnabled,
  subscribe,
  home,
  codexAccountsDir,
}) {
  registerBuiltinSkill({
    extensionId,
    name: NAME,
    text: fs.readFileSync(SKILL_PATH, "utf8"),
    isEnabled,
  });
  if (typeof subscribe === "function")
    subscribe((id) => {
      if (id === extensionId)
        syncLocalBuiltinSkills(home, undefined, codexAccountsDir);
    });
}

module.exports = { configureRoutinesSkill, ROUTINES_SKILL_NAME: NAME };
