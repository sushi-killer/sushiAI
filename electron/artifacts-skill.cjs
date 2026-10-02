const fs = require("node:fs");
const path = require("node:path");
const { quote } = require("./connections.cjs");

const NAME = "sushiai-artifacts";
const TOOLS = [".claude", ".codex"];
const SKILL_PATH = path.join(__dirname, "extensions", "artifacts-skill.md");

let isEnabled = () => true;
let warned = false;

/** Lets the host app say whether the Artifacts extension is on. */
function configureArtifactsSkill(options) {
  if (options && typeof options.isEnabled === "function")
    isEnabled = options.isEnabled;
}

function warnOnce(error) {
  if (warned) return;
  warned = true;
  console.warn("Artifacts skill install failed:", error?.message || error);
}

/** Never write a home from a test launch. */
function allowed(env = process.env) {
  return !env.SUSHIAI_TEST_WINDOW && isEnabled();
}

function skillText() {
  return fs.readFileSync(SKILL_PATH, "utf8");
}

/** Writes the skill into Claude Code's and Codex's home on this machine and
 * into every extra Codex home (one per Codex account), creating the folders,
 * so any agent sushiAI starts finds it. Writes only when the content differs.
 * Returns the files written. */
function installLocalArtifactsSkill(home, text = skillText(), codexHomes = []) {
  return writeSkill(
    [...TOOLS.map((tool) => path.join(home, tool)), ...codexHomes],
    text,
  );
}

function writeSkill(dirs, text) {
  const written = [];
  for (const dir of dirs) {
    const file = path.join(dir, "skills", NAME, "SKILL.md");
    try {
      if (fs.existsSync(file) && fs.readFileSync(file, "utf8") === text)
        continue;
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, text);
      written.push(file);
    } catch (error) {
      warnOnce(error);
    }
  }
  return written;
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

/** A POSIX shell script that does the same on a host; the content travels as
 * base64 so quotes, `$` and backticks need no escaping. */
function remoteInstallScript(text = skillText()) {
  const encoded = Buffer.from(text, "utf8").toString("base64");
  const tools = TOOLS.map((tool) => quote(tool)).join(" ");
  return `set -u
tmp="$(mktemp)" || exit 0
trap 'rm -f "$tmp"' EXIT
printf %s ${quote(encoded)} | { base64 -d 2>/dev/null || base64 -D; } > "$tmp" || exit 0
for tool in ${tools}; do
  d="$HOME/$tool/skills/${NAME}"
  cmp -s "$tmp" "$d/SKILL.md" 2>/dev/null && continue
  mkdir -p "$d" && cp "$tmp" "$d/SKILL.md"
done
exit 0
`;
}

/** Installs on an SSH host through the connection's own exec. Never throws. */
async function installRemoteArtifactsSkill(connections, endpoint, env) {
  if (!allowed(env)) return false;
  try {
    await connections.exec(endpoint, "sh -s", {
      input: remoteInstallScript(),
      timeout: 30000,
    });
    return true;
  } catch (error) {
    warnOnce(error);
    return false;
  }
}

/** Installs for this machine's home and its Codex accounts on app start.
 * Never throws. */
function installArtifactsSkillLocal(home, env, codexAccountsDir) {
  if (!allowed(env)) return [];
  try {
    return installLocalArtifactsSkill(
      home,
      skillText(),
      codexAccountsDir ? codexAccountHomes(codexAccountsDir) : [],
    );
  } catch (error) {
    warnOnce(error);
    return [];
  }
}

/** Installs into one new Codex account home. Never throws. */
function installArtifactsSkillInto(codexHome, env) {
  if (!allowed(env)) return [];
  try {
    return writeSkill([codexHome], skillText());
  } catch (error) {
    warnOnce(error);
    return [];
  }
}

module.exports = {
  configureArtifactsSkill,
  installLocalArtifactsSkill,
  installArtifactsSkillLocal,
  installArtifactsSkillInto,
  installRemoteArtifactsSkill,
  remoteInstallScript,
};
