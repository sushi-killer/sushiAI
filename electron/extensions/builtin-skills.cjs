const fs = require("node:fs");
const path = require("node:path");
const { REMOTE_BIN } = require("../host-install.cjs");
const { runSushiai } = require("../sushiai-run.cjs");

// One generic hook for skills a built-in extension ships. Core modules call
// only this file; each extension registers its own skill and says whether it
// is switched on. A disabled extension has its skill removed, never kept.
// The sushiai binary owns the skill folders (`sushiai skills install|remove`):
// this file only says which skill, with what text, on which home.
const skills = new Map();
const NAME = /^[a-z0-9][a-z0-9-]{0,63}$/;
// This machine: set by configureBuiltinSkillSync from the main process.
let local = {
  binary: () => null,
  codexAccountsDir: null,
  env: process.env,
  run: runSushiai,
};
let queue = Promise.resolve();
let warned = false;

/** Registers a skill (`<name>/SKILL.md` text) owned by a built-in extension. */
function registerBuiltinSkill({ extensionId, name, text, isEnabled }) {
  if (!extensionId || !NAME.test(name || "") || typeof text !== "string")
    throw new Error("A builtin skill needs extensionId, a plain name and text");
  skills.set(name, {
    extensionId,
    name,
    text,
    isEnabled: typeof isEnabled === "function" ? isEnabled : () => true,
  });
}

/** Where this machine's syncs go: `binary()` names the sushiai binary,
 * `codexAccountsDir` holds the Codex account homes (each gets its own run with
 * CODEX_HOME set). `run(binary, args, { input, env })` is replaceable in tests. */
function configureBuiltinSkillSync({
  binary,
  codexAccountsDir = null,
  env = process.env,
  run = runSushiai,
}) {
  local = { binary, codexAccountsDir, env, run };
}

function warnOnce(error) {
  if (warned) return;
  warned = true;
  console.warn("Builtin skill sync failed:", error?.message || error);
}

/** Never touch a home from a test launch. */
function active(env = process.env) {
  return !env.SUSHIAI_TEST_WINDOW;
}

/** The account homes under `dir` (`<userData>/codex-accounts/<id>`). */
function codexAccountHomes(dir) {
  try {
    return fs
      .readdirSync(dir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => path.join(dir, entry.name));
  } catch {
    return [];
  }
}

/** The argv and stdin that bring one skill in line with its extension. */
function skillCommand(skill, install = skill.isEnabled()) {
  return install
    ? { args: ["skills", "install", skill.name], input: skill.text }
    : { args: ["skills", "remove", skill.name], input: "" };
}

/** Runs `commands` on this machine, once per home: the default one and each
 * `codexHomes` entry as CODEX_HOME. Never throws; resolves the changed lines. */
async function runLocal(commands, codexHomes) {
  if (!active(local.env) || !commands.length) return [];
  let binary;
  try {
    binary = local.binary();
  } catch (error) {
    warnOnce(error);
    return [];
  }
  if (!binary) return [];
  const changed = [];
  for (const codexHome of codexHomes)
    for (const { args, input } of commands) {
      const env = codexHome
        ? { ...local.env, CODEX_HOME: codexHome }
        : local.env;
      try {
        const out = await local.run(binary, args, { input, env });
        changed.push(...String(out).split("\n").filter(Boolean));
      } catch (error) {
        warnOnce(error);
      }
    }
  return changed;
}

/** One sync at a time, in call order. */
function serialized(work) {
  const next = queue.then(work);
  queue = next.catch(() => {});
  return next;
}

/** Brings this machine's homes in line with the registry: installs the skill
 * of every enabled extension, removes that of every disabled one. */
function syncLocalBuiltinSkills() {
  return serialized(() =>
    runLocal(
      [...skills.values()].map((skill) => skillCommand(skill)),
      [
        null,
        ...(local.codexAccountsDir
          ? codexAccountHomes(local.codexAccountsDir)
          : []),
      ],
    ),
  );
}

/** Installs the enabled skills into one new Codex account home. */
function installBuiltinSkillsInto(codexHome) {
  return serialized(() =>
    runLocal(
      [...skills.values()]
        .filter((skill) => skill.isEnabled())
        .map((skill) => skillCommand(skill, true)),
      [codexHome],
    ),
  );
}

/** Installs or removes every registered skill on an SSH host with the host's
 * own sushiai binary, so it runs after that binary is installed. Never throws;
 * true when every command ran. */
async function syncBuiltinSkillsOnHost(connections, endpoint, env) {
  if (!active(env)) return false;
  let ok = true;
  for (const skill of skills.values()) {
    const { args, input } = skillCommand(skill);
    try {
      await connections.exec(
        endpoint,
        `"${REMOTE_BIN}/sushiai" ${args.join(" ")}`,
        { input, timeout: 30000 },
      );
    } catch (error) {
      ok = false;
      warnOnce(error);
    }
  }
  return ok;
}

module.exports = {
  registerBuiltinSkill,
  configureBuiltinSkillSync,
  installBuiltinSkillsInto,
  syncBuiltinSkillsOnHost,
  syncLocalBuiltinSkills,
  codexAccountHomes,
};
