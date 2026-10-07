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

/** The project of a folder and the values it sends to the host. */
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

/** What the app runs on a host to collect its sessions' logins (a login the
 * host refreshed is kept under `~/.sushiai/codex-sessions/<token>`, see
 * prepareRemoteCodexHome, at most seven days), one block per token: `@@ <token>`, then whether the session is `live` (the file is
 * read and left), `ended` (read and removed) or `gone`, then the file. A
 * session of another host sharing this home counts as live. */
const CODEX_COLLECT = (tokens) =>
  `me=$(uname -n); for t in ${tokens.join(" ")}; do d="$HOME/.sushiai/codex-sessions/$t"; echo "@@ $t"; if [ ! -d "$d" ]; then echo gone; continue; fi; p=$(cat "$d/pid" 2>/dev/null); h=$(cat "$d/host" 2>/dev/null); if [ ! -f "$d/.ret" ] && { { [ -n "$h" ] && [ "$h" != "$me" ]; } || { [ -n "$p" ] && kill -0 "$p" 2>/dev/null; } || { [ -z "$p" ] && [ -z "$(find "$d" -maxdepth 0 -mmin +5)" ]; }; }; then echo live; cat "$d/auth.json"; else echo ended; cat "$d/auth.json" 2>/dev/null; rm -rf "$d"; fi; echo; done`;

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

/** Who a Codex login belongs to: the ChatGPT account id or the API key; null
 * when it cannot be read (so it never matches another login). */
function loginIdentity(text) {
  try {
    const auth = JSON.parse(text);
    if (auth.OPENAI_API_KEY) return `key:${auth.OPENAI_API_KEY}`;
    if (auth.tokens?.account_id) return `account:${auth.tokens.account_id}`;
  } catch {
    // unreadable
  }
  return null;
}

const sameLogin = (a, b) => {
  const id = loginIdentity(a);
  return id !== null && id === loginIdentity(b);
};

/** The Codex home on a host for an account. The Mac login replaces the host's
 * when the host has none, when it is a later login, or when the host holds
 * another identity (a different account or API key). The host copy stays only
 * when it is a newer login of the same account: a host that refreshed its own
 * keeps it, and `ret` lets the app collect it. The login travels on stdin only. */
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
  const write =
    !hostAuth || newerLogin(auth, hostAuth) || !sameLogin(auth, hostAuth);
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
  sessionAccountId,
  codexAccountFor,
  CODEX_COLLECT,
};
