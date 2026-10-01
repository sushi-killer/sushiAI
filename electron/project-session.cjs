const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { quote } = require("./connections.cjs");

/** The project a folder on a host belongs to: the one the folder was attached
 * to, else the one of its git remote. Null when it has none. */
async function projectForFolder({ projects, connections }, endpoint, cwd) {
  if (!projects || typeof cwd !== "string" || !cwd) return null;
  const host =
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? endpoint
      : "local";
  const byFolder = await projects.resolveFolder({ endpoint: host, cwd });
  if (byFolder) return byFolder;
  const info = await connections
    ?.inspect(host === "local" ? undefined : host, {
      operation: "git_remote",
      root: cwd,
    })
    .catch(() => null);
  return info?.remote ? projects.resolve(info.remote) : null;
}

/** Exported lines for a shell to source: one `export NAME='value'` each. */
function envPayload(env) {
  return Object.entries(env || {})
    .map(([name, value]) => `export ${name}=${quote(value)}`)
    .join("\n");
}

/** Codex signs in through `auth.json` in its home. A host with no login of
 * its own gets the local one, the way Codex's docs sign in a headless machine;
 * a host that has one keeps it. The file goes over ssh stdin, never argv. */
const SEED_CODEX =
  'd="${CODEX_HOME:-$HOME/.codex}"; [ -s "$d/auth.json" ] && exit 0; umask 077; mkdir -p "$d" && cat > "$d/auth.json.tmp" && mv "$d/auth.json.tmp" "$d/auth.json"';
async function seedCodexLogin(connections, endpoint, codexHome) {
  if (typeof endpoint !== "string" || !endpoint.startsWith("ssh:")) return;
  const home = codexHome || path.join(os.homedir(), ".codex");
  const auth = await fs
    .readFile(path.join(home, "auth.json"), "utf8")
    .catch(() => "");
  if (auth.trim())
    await connections.exec(endpoint, SEED_CODEX, { input: auth });
}

/** The variables a Claude account is used through: a subscription's OAuth
 * token, or an API key. */
function accountVars(account) {
  if (!account?.value) return {};
  return account.kind === "subscription"
    ? { CLAUDE_CODE_OAUTH_TOKEN: account.value }
    : { ANTHROPIC_API_KEY: account.value };
}

/** The Claude account a session on a host runs as: the one picked for it,
 * else the project's own. A host switched off for the folder's project gets
 * none, like it gets none of the project's values. */
function sessionAccountId(project, sends, picked) {
  if (project && !sends) return undefined;
  return picked || project?.sessions?.claudeAccount || undefined;
}

/** The command a remote pane starts Claude on a custom model with. The
 * settings carry no secret, so they ride inline on the command line; the
 * key is in the shell's environment (sent like a project value) and
 * apiKeyHelper reads it from there - no file has to outlive the start. */
function remoteModelLaunch(settings) {
  const document = {
    apiKeyHelper: 'printf %s "$SUSHIAI_MODEL_KEY"',
    env: settings,
  };
  return `claude --settings ${quote(JSON.stringify(document))}`;
}

/** What to type into a Herdr pane so the project's values and the session's
 * Claude account are in its shell: they go into a one-shot 0600 file (on the
 * host, over ssh stdin; here for the local machine), and the text typed only sources
 * and removes that file, so no value is ever in the text, the pane's history
 * or argv. Empty when there is nothing to send. */
async function sessionEnvPrefix(
  { projects, connections, upload, remove, resolveAccount, resolveModel },
  { endpoint, cwd, claudeAccountId, agent, modelProfileId },
) {
  const project = await projectForFolder(
    { projects, connections },
    endpoint,
    cwd,
  );
  const remote = typeof endpoint === "string" && endpoint.startsWith("ssh:");
  const host = remote ? endpoint : "local";
  const sends = project ? await projects.sendsValues(project.id, host) : false;
  const vars = sends
    ? await projects.environmentFor(project.id, "agent", host)
    : {};
  // A shell may run Claude later; another agent's pane never needs it.
  const accountId =
    !agent || agent === "claude"
      ? sessionAccountId(project, sends, claudeAccountId)
      : undefined;
  if (accountId && resolveAccount)
    Object.assign(vars, accountVars(await resolveAccount(accountId)));
  // A custom model's key, for the pane `remoteModelLaunch` starts.
  if (modelProfileId && remote && resolveModel)
    vars.SUSHIAI_MODEL_KEY = (await resolveModel(modelProfileId)).key;
  const payload = envPayload(vars);
  if (!payload) return "";
  let file;
  if (remote) {
    file = await upload(endpoint, payload);
    // A file nothing sourced does not stay: gone after two minutes.
    const timer = setTimeout(() => void remove(endpoint, file), 120000);
    timer.unref?.();
  } else {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-env-"));
    file = path.join(dir, "env");
    await fs.writeFile(file, payload, { mode: 0o600 });
    const timer = setTimeout(
      () => void fs.rm(dir, { recursive: true, force: true }),
      120000,
    );
    timer.unref?.();
    return `. ${quote(file)}; rm -rf ${quote(dir)}; `;
  }
  return `. ${quote(file)}; rm -f ${quote(file)}; `;
}

module.exports = {
  projectForFolder,
  sessionEnvPrefix,
  sessionAccountId,
  accountVars,
  seedCodexLogin,
  remoteModelLaunch,
  envPayload,
};
