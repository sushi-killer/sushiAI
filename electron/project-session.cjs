const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { quote } = require("./connections.cjs");
const { normalizeRemote } = require("./projects.cjs");

/** The project a folder on a host belongs to: the one the folder was attached
 * to, else the one of its git remote. Null when it has none. */
async function projectForFolder(
  { projects, connections, strict = false },
  endpoint,
  cwd,
) {
  if (!projects || typeof cwd !== "string" || !cwd) return null;
  const host =
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? endpoint
      : "local";
  const attached = await projects.resolveProject({ host, cwd });
  if (attached) return attached;
  const info = await connections
    ?.inspect(host === "local" ? undefined : host, {
      operation: "git_remote",
      root: cwd,
    })
    .catch((error) => {
      if (strict) throw error;
      return null;
    });
  return info
    ? projects.resolveProject({
        host,
        cwd,
        remoteKey: normalizeRemote(info.remote),
        commonDir: info.commonDir,
      })
    : null;
}

/** Project values travel in the Herdr creation RPC; account files are prepared separately. */
async function sessionEnvironment(
  { projects, connections },
  { endpoint, cwd },
) {
  const project = await projectForFolder(
    { projects, connections, strict: true },
    endpoint,
    cwd,
  );
  const host =
    typeof endpoint === "string" && endpoint.startsWith("ssh:")
      ? endpoint
      : "local";
  const sends = project ? await projects.sendsValues(project.id, host) : false;
  return {
    project,
    sends,
    withheld: Boolean(project) && !sends,
    env: sends ? await projects.environmentFor(project.id, "agent", host) : {},
  };
}

/** Exported lines for a shell to source: one `export NAME='value'` each. */
function envPayload(env) {
  return Object.entries(env || {})
    .map(([name, value]) => `export ${name}=${quote(value)}`)
    .join("\n");
}

/** Codex signs in through `auth.json` in its home. A host with no login of
 * its own runs a session on the local one, for that session only: the file
 * comes in the one-shot values file (as SUSHIAI_CODEX_AUTH), goes into a
 * private temporary Codex home with the host's own config and history linked
 * in, and is removed when Codex exits or its pane closes. Herdr closes a
 * pane with SIGKILL, which no trap sees: each start also clears the homes of
 * sessions whose shell is gone. A host that has a login keeps using it,
 * unless the session was started as a picked Codex account
 * (SUSHIAI_CODEX_FORCE).
 *
 * A ChatGPT account's refresh token is single-use, so a session that
 * refreshed it must hand the new login back: started with
 * SUSHIAI_CODEX_RETURN, its home has a name the app knows
 * (`~/.sushiai/codex-sessions/<token>`, see CODEX_COLLECT), and when it ends
 * only a changed `auth.json` stays there (marked `.ret`) until the app
 * collects it, at most seven days. An unchanged one is removed like any
 * other session home. */
const CODEX_SESSION = [
  'keep() { [ -f "$1/.ret" ] && return; if [ -f "$1/.sent" ] && [ "$(cat "$1/auth.json" 2>/dev/null)" != "$(cat "$1/.sent")" ]; then for f in "$1"/* "$1"/.[!.]*; do { [ -e "$f" ] || [ -L "$f" ]; } && [ "${f##*/}" != auth.json ] && rm -rf "$f"; done; : > "$1/.ret"; else rm -rf "$1"; fi; }',
  // A home shared between hosts (NFS) holds other hosts' sessions: their
  // pid means nothing here, so only this host's are judged.
  'me=$(uname -n); for o in "${TMPDIR:-/tmp}"/sushiai-codex.* "$HOME"/.sushiai/codex-sessions/*; do [ -d "$o" ] || continue; if [ -f "$o/.ret" ]; then [ -n "$(find "$o/.ret" -mtime +7)" ] && rm -rf "$o"; continue; fi; h=$(cat "$o/host" 2>/dev/null); [ -n "$h" ] && [ "$h" != "$me" ] && continue; p=$(cat "$o/pid" 2>/dev/null); [ -n "$p" ] && kill -0 "$p" 2>/dev/null || keep "$o"; done',
  "force=${SUSHIAI_CODEX_FORCE:-}; t=${SUSHIAI_CODEX_RETURN:-}; unset SUSHIAI_CODEX_FORCE SUSHIAI_CODEX_RETURN",
  'if { [ -z "$force" ] && [ -s "${CODEX_HOME:-$HOME/.codex}/auth.json" ]; } || [ -z "${SUSHIAI_CODEX_AUTH:-}" ]; then unset SUSHIAI_CODEX_AUTH; exec codex; fi',
  'c="$HOME/.codex"; mkdir -p "$c/sessions"',
  'if [ -n "$t" ]; then mkdir -p -m 700 "$HOME/.sushiai/codex-sessions"; d="$HOME/.sushiai/codex-sessions/$t"; mkdir -m 700 "$d" || exit 1; else d=$(mktemp -d "${TMPDIR:-/tmp}/sushiai-codex.XXXXXX"); fi; echo $$ > "$d/pid"; uname -n > "$d/host"',
  "trap 'keep \"$d\"' EXIT HUP INT TERM",
  'for f in "$c"/* "$c"/.[!.]*; do [ -e "$f" ] && [ "${f##*/}" != auth.json ] && [ "${f##*/}" != pid ] && ln -s "$f" "$d/"; done',
  '(umask 077; printf %s "$SUSHIAI_CODEX_AUTH" > "$d/auth.json"; [ -z "$t" ] || cp "$d/auth.json" "$d/.sent"); unset SUSHIAI_CODEX_AUTH',
  'CODEX_HOME="$d" codex',
].join("; ");
/** What the app runs on a host to collect its sessions' logins, one block
 * per token: `@@ <token>`, then whether the session is `live` (the file is
 * read and left), `ended` (read and removed) or `gone`, then the file. A
 * session of another host sharing this home counts as live. */
const CODEX_COLLECT = (tokens) =>
  `me=$(uname -n); for t in ${tokens.join(" ")}; do d="$HOME/.sushiai/codex-sessions/$t"; echo "@@ $t"; if [ ! -d "$d" ]; then echo gone; continue; fi; p=$(cat "$d/pid" 2>/dev/null); h=$(cat "$d/host" 2>/dev/null); if [ ! -f "$d/.ret" ] && { { [ -n "$h" ] && [ "$h" != "$me" ]; } || { [ -n "$p" ] && kill -0 "$p" 2>/dev/null; } || { [ -z "$p" ] && [ -z "$(find "$d" -maxdepth 0 -mmin +5)" ]; }; }; then echo live; cat "$d/auth.json"; else echo ended; cat "$d/auth.json" 2>/dev/null; rm -rf "$d"; fi; echo; done`;

const codexSessionLaunch = () => `sh -c ${quote(CODEX_SESSION)}`;

/** The local Codex login, or "" when there is none. */
function localCodexAuth(
  codexHome = path.join(os.homedir(), ".codex"),
  strict = false,
) {
  return fs.readFile(path.join(codexHome, "auth.json"), "utf8").then(
    (text) => text.trim(),
    (error) => {
      if (strict && error.code !== "ENOENT") throw error;
      return "";
    },
  );
}

/** How a Claude account reaches a session: the variables to send and what
 * to add to the `claude` command. A subscription's OAuth token is read from
 * its variable; an API key goes through apiKeyHelper, because interactive
 * Claude Code drops an env API key the user never approved (see
 * `stageSettings` in model-providers.cjs). */
function accountLaunch(account) {
  if (!account?.value) return { vars: {}, settings: "" };
  if (account.kind === "subscription")
    return { vars: { CLAUDE_CODE_OAUTH_TOKEN: account.value }, settings: "" };
  const document = { apiKeyHelper: 'printf %s "$SUSHIAI_ACCOUNT_KEY"' };
  return {
    vars: { SUSHIAI_ACCOUNT_KEY: account.value },
    settings: ` --settings ${quote(JSON.stringify(document))}`,
  };
}

/** The account a session on a host runs as: the one picked for it, else the
 * project's own (`sessions.claudeAccount` or `sessions.codexAccount`). An
 * empty pick is "none" (the host's own login). A host switched off for the
 * folder's project gets none, like it gets none of the project's values. */
function sessionAccountId(project, sends, picked, key = "claudeAccount") {
  if (picked === "" || (project && !sends)) return undefined;
  return picked || project?.sessions?.[key] || undefined;
}

/** A Codex account, or null for the host's own login. The project's own
 * account that is not signed in yet falls back to it; a picked one has to
 * work. */
async function codexAccountFor(
  resolve,
  accountId,
  picked,
  endpoint,
  strict = false,
) {
  if (!accountId || !resolve) return null;
  return resolve(accountId, endpoint).catch((error) => {
    if (picked || (strict && error.code !== "ACCOUNT_NOT_CONFIGURED"))
      throw error;
    return null;
  });
}

/** How Codex starts on the local machine as an account: in its own home. */
const codexHomeLaunch = (home) => `env CODEX_HOME=${quote(home)} codex`;

/** The command a Herdr pane starts Claude on a custom model with, on any
 * host. The settings carry no secret, so they ride inline on the command
 * line; the key is in the agent's environment (sent like a project value)
 * and apiKeyHelper reads it from there - no file outlives the start. */
function modelLaunch(settings) {
  const document = {
    apiKeyHelper: 'printf %s "$SUSHIAI_MODEL_KEY"',
    env: settings,
  };
  return `claude --settings ${quote(JSON.stringify(document))}`;
}

/** What to type into a Herdr pane so the project's values and the session's
 * sign-in are in its shell: they go into a one-shot 0600 file (on the host,
 * over ssh stdin; here for the local machine), and the text typed only
 * sources and removes that file, so no value is ever in the text, the pane's
 * history or argv. `settings` is what the `claude` command needs added for
 * the account, `launch` a command to start the agent with instead of its
 * name. All empty when there is nothing to send. */
async function sessionEnvPrefix(
  {
    projects,
    connections,
    upload,
    remove,
    resolveAccount,
    resolveCodexAccount,
    resolveModel,
    codexAuth = localCodexAuth,
  },
  {
    endpoint,
    cwd,
    claudeAccountId,
    codexAccountId,
    agent,
    modelProfileId,
    nativeEnvironment,
  },
) {
  const project = nativeEnvironment
    ? nativeEnvironment.project
    : await projectForFolder({ projects, connections }, endpoint, cwd);
  const remote = typeof endpoint === "string" && endpoint.startsWith("ssh:");
  const host = remote ? endpoint : "local";
  const sends = nativeEnvironment
    ? nativeEnvironment.sends
    : project
      ? await projects.sendsValues(project.id, host)
      : false;
  const withheld = Boolean(project) && !sends;
  const vars =
    sends && !nativeEnvironment
      ? await projects.environmentFor(project.id, "agent", host)
      : {};
  let settings = "";
  let launch = "";
  if (agent === "codex" && !withheld) {
    const account = await codexAccountFor(
      resolveCodexAccount,
      sessionAccountId(project, sends, codexAccountId, "codexAccount"),
      codexAccountId,
      remote ? endpoint : undefined,
      Boolean(nativeEnvironment),
    );
    const auth = remote
      ? account?.auth ||
        (await codexAuth(undefined, Boolean(nativeEnvironment)))
      : "";
    if (account && !remote) launch = codexHomeLaunch(account.home);
    else if (auth) {
      vars.SUSHIAI_CODEX_AUTH = auth;
      if (account) vars.SUSHIAI_CODEX_FORCE = "1";
      if (account?.ret) vars.SUSHIAI_CODEX_RETURN = account.ret;
      launch = codexSessionLaunch();
    }
  }
  // Only Claude itself, and not on a custom model: that has its own key, and
  // an account's key must never reach the model's gateway.
  const accountId =
    agent === "claude" && !modelProfileId
      ? sessionAccountId(project, sends, claudeAccountId)
      : undefined;
  if (accountId && resolveAccount) {
    // The project's own account without a value yet: the session still gets
    // the project's values, on the host's own login.
    const account = await resolveAccount(accountId).catch((error) => {
      if (
        claudeAccountId ||
        (nativeEnvironment && error.code !== "ACCOUNT_NOT_CONFIGURED")
      )
        throw error;
      return null;
    });
    const launch = accountLaunch(account);
    Object.assign(vars, launch.vars);
    settings = launch.settings;
  }
  // A custom model's key, for the pane `modelLaunch` starts.
  if (modelProfileId && resolveModel)
    vars.SUSHIAI_MODEL_KEY = (await resolveModel(modelProfileId)).key;
  const payload = envPayload(vars);
  if (!payload) return { prefix: "", settings, launch };
  if (remote) {
    const file = await upload(endpoint, payload);
    if (
      nativeEnvironment &&
      (typeof file !== "string" ||
        !path.posix.isAbsolute(file) ||
        /[\r\n\0]/.test(file))
    )
      throw new Error("The host did not confirm the session preparation file.");
    // A file nothing sourced does not stay: gone after two minutes.
    const timer = setTimeout(() => void remove(endpoint, file), 120000);
    timer.unref?.();
    return {
      prefix: `. ${quote(file)}${nativeEnvironment ? " || exit" : ""}; rm -f ${quote(file)}; `,
      settings,
      launch,
    };
  }
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-env-"));
  const file = path.join(dir, "env");
  await fs.writeFile(file, payload, { mode: 0o600 });
  const timer = setTimeout(
    () => void fs.rm(dir, { recursive: true, force: true }),
    120000,
  );
  timer.unref?.();
  return {
    prefix: `. ${quote(file)}${nativeEnvironment ? " || exit" : ""}; rm -rf ${quote(dir)}; `,
    settings,
    launch,
  };
}

const SAFE_ID = /^[A-Za-z0-9_-]+$/;

// Prepares a Codex account's home on a host: ~/.sushiai/codex-accounts/<id>
// with the host's own ~/.codex linked in (like the local account home), and
// the account's auth.json from stdin when `write` is "1". Hooks and trust are
// the daemon's job. A return token `$2` links ~/.sushiai/codex-sessions/<token>
// to the home, which is where CODEX_COLLECT looks for a login the host
// refreshed. Prints the absolute home.
const remoteCodexHomeScript = (id) =>
  [
    "umask 077",
    `h="$HOME/.sushiai/codex-accounts"/${quote(id)}`,
    'mkdir -p "$h" || exit 1',
    'chmod 700 "$h"',
    'c="$HOME/.codex"; mkdir -p "$c/sessions"',
    'for f in "$c"/* "$c"/.[!.]*; do { [ -e "$f" ] || [ -L "$f" ]; } || continue; n=${f##*/}; [ "$n" = auth.json ] && continue; [ -e "$h/$n" ] || [ -L "$h/$n" ] || ln -s "$f" "$h/$n"; done',
    'if [ "$1" = 1 ]; then cat > "$h/auth.json.new" && mv "$h/auth.json.new" "$h/auth.json"; else cat >/dev/null; fi',
    'if [ -n "$2" ]; then mkdir -p "$HOME/.sushiai/codex-sessions" && ln -sfn "$h" "$HOME/.sushiai/codex-sessions/$2"; fi',
    "printf '%s\\n' \"$h\"",
  ].join("; ");

/** The Codex home on a host for an account. The Mac login replaces the host's
 * only when it is a later login of the same ChatGPT account (or the host has
 * none): a host that refreshed its own keeps it, and `ret` lets the app
 * collect it. The login travels on stdin only. */
async function prepareRemoteCodexHome({
  exec,
  endpoint,
  accountId,
  auth,
  ret,
}) {
  if (!exec) throw new Error("This host cannot prepare a Codex account.");
  if (!SAFE_ID.test(accountId || "")) throw new Error("Invalid Codex account.");
  if (ret !== undefined && !SAFE_ID.test(ret))
    throw new Error("Invalid return token.");
  if (typeof auth !== "string" || !auth)
    throw new Error("The Codex account has no login to place on the host.");
  const { newerLogin } = require("./codex-accounts.cjs");
  const hostAuth = String(
    await exec(
      endpoint,
      `sh -c ${quote('cat "$HOME/.sushiai/codex-accounts"/"$1"/auth.json 2>/dev/null; true')} sh ${quote(accountId)}`,
      { timeout: 30000 },
    ),
  ).trim();
  const write = !hostAuth || newerLogin(auth, hostAuth);
  const out = String(
    await exec(
      endpoint,
      `sh -c ${quote(remoteCodexHomeScript(accountId))} sh ${write ? 1 : 0} ${quote(ret || "")}`,
      { input: write ? auth : "", timeout: 30000 },
    ),
  ).trim();
  if (!path.posix.isAbsolute(out) || /[\r\n\0]/.test(out))
    throw new Error("The host did not confirm the Codex home.");
  return out;
}

/** What a daemon session needs besides its command: process variables and
 * the Claude `--settings` keys the daemon allows (`apiKeyHelper`, `model`).
 * Every secret rides in `env`; claudeSettings holds only a helper command that
 * reads a key from an env variable. `host` is "local" or a connection id; a
 * Codex account on a remote host gets its home prepared there over `exec`.
 * Error messages never carry a value. */
async function sessionLaunchEnv(
  {
    projects,
    connections,
    exec,
    resolveAccount,
    resolveCodexAccount,
    resolveModel,
  },
  { host, cwd, agent, claudeAccountId, codexAccountId, modelProfileId },
) {
  const remote = host !== "local";
  const endpoint = remote ? `ssh:${host}` : undefined;
  // A command connector has no shell to inspect folders with.
  const inspectable =
    !remote || !connections?.hasShell || connections.hasShell(endpoint);
  const environment = await sessionEnvironment(
    { projects, connections: inspectable ? connections : undefined },
    { endpoint, cwd },
  );
  const { project, sends, withheld } = environment;
  const env = { ...environment.env };
  const claudeSettings = {};
  if (agent === "codex" && !withheld) {
    const accountId = sessionAccountId(
      project,
      sends,
      codexAccountId,
      "codexAccount",
    );
    const account = await codexAccountFor(
      resolveCodexAccount,
      accountId,
      codexAccountId,
      endpoint,
      true,
    );
    if (account && remote)
      env.CODEX_HOME = await prepareRemoteCodexHome({
        exec,
        endpoint,
        accountId,
        auth: account.auth,
        ret: account.ret,
      });
    else if (account) {
      if (!path.isAbsolute(account.home))
        throw new Error("The Codex account home is not an absolute path.");
      env.CODEX_HOME = account.home;
    }
  }
  if (agent === "claude" && modelProfileId) {
    const model = await resolveModel(modelProfileId);
    Object.assign(env, model.settings, { CLAUDE_HELPER_MODEL_KEY: model.key });
    claudeSettings.apiKeyHelper = 'printf %s "$CLAUDE_HELPER_MODEL_KEY"';
  } else if (agent === "claude") {
    const accountId = sessionAccountId(project, sends, claudeAccountId);
    if (accountId && resolveAccount) {
      const account = await resolveAccount(accountId).catch((error) => {
        if (claudeAccountId || error.code !== "ACCOUNT_NOT_CONFIGURED")
          throw error;
        return null;
      });
      if (account?.value && account.kind === "subscription")
        env.CLAUDE_CODE_OAUTH_TOKEN = account.value;
      else if (account?.value) {
        env.CLAUDE_HELPER_ACCOUNT_KEY = account.value;
        claudeSettings.apiKeyHelper = 'printf %s "$CLAUDE_HELPER_ACCOUNT_KEY"';
      }
    }
  }
  return { env, claudeSettings, project };
}

module.exports = {
  sessionLaunchEnv,
  prepareRemoteCodexHome,
  projectForFolder,
  sessionEnvironment,
  sessionEnvPrefix,
  sessionAccountId,
  codexAccountFor,
  codexHomeLaunch,
  accountLaunch,
  CODEX_SESSION,
  CODEX_COLLECT,
  localCodexAuth,
  modelLaunch,
  envPayload,
};
