const fs = require("node:fs");
const path = require("node:path");
const { quote } = require("../connections.cjs");

// One generic hook for skills a built-in extension ships. Core modules call
// only this file; each extension registers its own skill and says whether it
// is switched on. A disabled extension has its skill removed, never kept.
const TOOLS = [".claude", ".codex"];
const skills = new Map();
let warned = false;

/** Registers a skill (`<name>/SKILL.md` text) owned by a built-in extension. */
function registerBuiltinSkill({ extensionId, name, text, isEnabled }) {
  if (!extensionId || !name || typeof text !== "string")
    throw new Error("A builtin skill needs extensionId, name and text");
  skills.set(name, {
    extensionId,
    name,
    text,
    isEnabled: typeof isEnabled === "function" ? isEnabled : () => true,
  });
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

function declares(name, content) {
  return new RegExp(`^name:\\s*${name.replace(/\W/g, "\\$&")}\\s*$`, "m").test(
    content,
  );
}

/** Writes the skill into each tool home, creating folders; skips a file that
 * already has the content. Returns the files written. */
function writeSkill(name, homes, text) {
  const written = [];
  for (const dir of homes) {
    const file = path.join(dir, "skills", name, "SKILL.md");
    try {
      if (fs.existsSync(file)) {
        const current = fs.readFileSync(file, "utf8");
        // Same content, or a file that is not ours: leave it.
        if (current === text || !declares(name, current)) continue;
      }
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, text);
      written.push(file);
    } catch (error) {
      warnOnce(error);
    }
  }
  return written;
}

/** Removes `<home>/skills/<name>` only when its SKILL.md declares this name;
 * any other folder or file stays. Returns the files removed. */
function removeSkill(name, homes) {
  const removed = [];
  for (const dir of homes) {
    const folder = path.join(dir, "skills", name);
    const file = path.join(folder, "SKILL.md");
    try {
      if (!declares(name, fs.readFileSync(file, "utf8"))) continue;
      fs.rmSync(file);
      removed.push(file);
      try {
        fs.rmdirSync(folder);
      } catch {
        // Not empty: someone else's files stay.
      }
    } catch (error) {
      if (error?.code !== "ENOENT") warnOnce(error);
    }
  }
  return removed;
}

function localHomes(home, codexAccountsDir) {
  return [
    ...TOOLS.map((tool) => path.join(home, tool)),
    ...(codexAccountsDir ? codexAccountHomes(codexAccountsDir) : []),
  ];
}

/** Brings this machine's homes in line with the registry: installs the skill
 * of every enabled extension, removes that of every disabled one. */
function syncLocalBuiltinSkills(home, env, codexAccountsDir) {
  if (!active(env)) return [];
  const homes = localHomes(home, codexAccountsDir);
  const changed = [];
  for (const skill of skills.values()) {
    try {
      changed.push(
        ...(skill.isEnabled()
          ? writeSkill(skill.name, homes, skill.text)
          : removeSkill(skill.name, homes)),
      );
    } catch (error) {
      warnOnce(error);
    }
  }
  return changed;
}

/** Installs the enabled skills into one new Codex account home. */
function installBuiltinSkillsInto(codexHome, env) {
  if (!active(env)) return [];
  const written = [];
  for (const skill of skills.values())
    if (skill.isEnabled())
      written.push(...writeSkill(skill.name, [codexHome], skill.text));
  return written;
}

/** A POSIX shell script that installs the skill on a host; the content
 * travels as base64 so quotes, `$` and backticks need no escaping. */
function remoteInstallScript(name, text) {
  const encoded = Buffer.from(text, "utf8").toString("base64");
  const tools = TOOLS.map((tool) => quote(tool)).join(" ");
  return `set -u
tmp="$(mktemp)" || exit 0
trap 'rm -f "$tmp"' EXIT
printf %s ${quote(encoded)} | { base64 -d 2>/dev/null || base64 -D; } > "$tmp" || exit 0
for tool in ${tools}; do
  d="$HOME/$tool/skills/${name}"
  cmp -s "$tmp" "$d/SKILL.md" 2>/dev/null && continue
  mkdir -p "$d" && cp "$tmp" "$d/SKILL.md"
done
exit 0
`;
}

/** A POSIX shell script that removes the skill from a host, only where its
 * SKILL.md declares this name. */
function remoteRemoveScript(name) {
  const tools = TOOLS.map((tool) => quote(tool)).join(" ");
  return `set -u
for tool in ${tools}; do
  d="$HOME/$tool/skills/${name}"
  grep -qx ${quote(`name: ${name}`)} "$d/SKILL.md" 2>/dev/null || continue
  rm -f "$d/SKILL.md"
  rmdir "$d" 2>/dev/null
done
exit 0
`;
}

/** Installs or removes every registered skill on an SSH host through the
 * connection's own exec. Never throws; true when every script ran. */
async function syncBuiltinSkillsOnHost(connections, endpoint, env) {
  if (!active(env)) return false;
  let ok = true;
  for (const skill of skills.values()) {
    try {
      await connections.exec(endpoint, "sh -s", {
        input: skill.isEnabled()
          ? remoteInstallScript(skill.name, skill.text)
          : remoteRemoveScript(skill.name),
        timeout: 30000,
      });
    } catch (error) {
      ok = false;
      warnOnce(error);
    }
  }
  return ok;
}

module.exports = {
  registerBuiltinSkill,
  installBuiltinSkillsInto,
  syncBuiltinSkillsOnHost,
  syncLocalBuiltinSkills,
  writeSkill,
  removeSkill,
  codexAccountHomes,
  remoteInstallScript,
  remoteRemoveScript,
};
