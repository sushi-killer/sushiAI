// Codex accounts. Codex signs in through `auth.json` in its home and refreshes
// that file itself (a ChatGPT login's refresh token is single-use), so a copy
// kept elsewhere goes stale: each account is its own Codex home, signed in by
// `codex login` and left to Codex. Everything else in `~/.codex` (config,
// skills, sessions, history) is linked in, so every account shares it. The
// file at rest is what Codex keeps for its own default login; the list only
// ever returns who is signed in, never the login itself.
const { randomUUID } = require("node:crypto");
const { spawn } = require("node:child_process");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");

const { CODEX_COLLECT } = require("./project-session.cjs");

const LOGIN_TIMEOUT = 10 * 60 * 1000;
// How long a session not seen live is still asked for its login; the host
// keeps it as long (CODEX_SESSION).
const RETURN_DAYS = 7 * 24 * 60 * 60 * 1000;

/** True when `text` is a later login of the same ChatGPT account. */
function newerLogin(text, than) {
  try {
    const next = JSON.parse(text);
    const now = JSON.parse(than);
    return (
      Boolean(next.tokens?.refresh_token) &&
      next.tokens.account_id === now.tokens?.account_id &&
      Date.parse(next.last_refresh) > Date.parse(now.last_refresh)
    );
  } catch {
    return false;
  }
}

function signedInAs(text) {
  try {
    const auth = JSON.parse(text);
    if (auth.OPENAI_API_KEY)
      return { mode: "apiKey", detail: `…${auth.OPENAI_API_KEY.slice(-4)}` };
    const payload = auth.tokens?.id_token?.split(".")[1];
    if (!payload) return null;
    const claims = JSON.parse(Buffer.from(payload, "base64url").toString());
    return { mode: "chatgpt", detail: String(claims.email || "") };
  } catch {
    return null;
  }
}

class CodexAccounts {
  constructor({
    userDataDir,
    codexBinary,
    remoteExec,
    codexHome = path.join(os.homedir(), ".codex"),
  }) {
    this.remoteExec = remoteExec;
    this.file = path.join(userDataDir, "codex-accounts.json");
    this.homes = path.join(userDataDir, "codex-accounts");
    this.codexBinary = codexBinary;
    this.codexHome = codexHome;
    this.logins = new Map();
    this.queue = Promise.resolve();
  }

  /** A store that cannot be read is an error, never an empty one a write
   * would then replace. */
  async #read() {
    try {
      return JSON.parse(await fs.readFile(this.file, "utf8"));
    } catch (error) {
      if (error.code === "ENOENT") return {};
      throw error;
    }
  }
  #locked(change) {
    const run = this.queue.then(change);
    this.queue = run.catch(() => {});
    return run;
  }
  async #write(accounts) {
    await fs.mkdir(path.dirname(this.file), { recursive: true });
    await fs.writeFile(this.file, JSON.stringify(accounts, null, 2), {
      mode: 0o600,
    });
  }
  // An id becomes a folder name: only one this store made is ever used.
  async #account(id) {
    const accounts = await this.#read();
    if (typeof id !== "string" || !Object.hasOwn(accounts, id))
      throw new Error("Unknown Codex account.");
    return accounts[id];
  }
  homeFor(id) {
    return path.join(this.homes, id);
  }
  #auth(id) {
    return fs
      .readFile(path.join(this.homeFor(id), "auth.json"), "utf8")
      .catch(() => "");
  }

  async list() {
    const accounts = await this.#read();
    return Promise.all(
      Object.values(accounts).map(async (account) => {
        const who = signedInAs(await this.#auth(account.id));
        return {
          ...account,
          signedIn: Boolean(who),
          mode: who?.mode || "",
          detail: who?.detail || "",
          signingIn: this.logins.has(account.id),
        };
      }),
    );
  }

  add(label) {
    return this.#locked(() => this.#add(label));
  }
  async #add(label) {
    const accounts = await this.#read();
    const id = randomUUID();
    accounts[id] = {
      id,
      label: (
        (typeof label === "string" && label.trim()) ||
        "Codex account"
      ).slice(0, 80),
    };
    await fs.mkdir(this.homeFor(id), { recursive: true, mode: 0o700 });
    await this.#write(accounts);
    return { ...accounts[id], signedIn: false, mode: "", detail: "" };
  }

  remove(id) {
    return this.#locked(() => this.#remove(id));
  }
  async #remove(id) {
    await this.#account(id);
    const accounts = await this.#read();
    this.logins.get(id)?.kill();
    delete accounts[id];
    await this.#write(accounts);
    await fs.rm(this.homeFor(id), { recursive: true, force: true });
  }

  /** Signs an account in with `codex login`: in the browser, or with an API
   * key read from stdin. Resolves once Codex has written the login. */
  async login(id, apiKey) {
    const account = await this.#account(id);
    if (this.logins.has(id))
      throw new Error(`${account.label} is already signing in.`);
    // A browser sign-in listens on one fixed local port.
    if (
      apiKey === undefined &&
      [...this.logins.values()].some((p) => p.browser)
    )
      throw new Error("Finish the other browser sign-in first.");
    if (apiKey !== undefined && (typeof apiKey !== "string" || !apiKey.trim()))
      throw new Error("Paste an OpenAI API key.");
    const binary = this.codexBinary();
    if (!binary)
      throw new Error("Codex is not installed. Install it to sign in.");
    const home = this.homeFor(id);
    await fs.mkdir(home, { recursive: true, mode: 0o700 });
    const proc = spawn(
      binary,
      ["login", ...(apiKey === undefined ? [] : ["--with-api-key"])],
      {
        env: { ...process.env, CODEX_HOME: home },
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
    proc.browser = apiKey === undefined;
    this.logins.set(id, proc);
    let output = "";
    proc.stdout.on("data", (chunk) => (output += chunk));
    proc.stderr.on("data", (chunk) => (output += chunk));
    proc.stdin.end(apiKey === undefined ? "" : apiKey.trim());
    // ponytail: an abandoned browser sign-in ends here, not by a cancel button.
    const timer = setTimeout(() => proc.kill(), LOGIN_TIMEOUT);
    try {
      const code = await new Promise((resolve, reject) => {
        proc.on("error", reject);
        proc.on("close", resolve);
      });
      if (code !== 0 || !signedInAs(await this.#auth(id)))
        throw new Error(
          output.trim().split("\n").pop() ||
            `${account.label} did not sign in.`,
        );
    } finally {
      clearTimeout(timer);
      this.logins.delete(id);
    }
    // A new sign-in replaces whatever host sessions still hold.
    await this.#locked(async () => {
      const accounts = await this.#read();
      if (!Object.hasOwn(accounts, id)) return;
      accounts[id].returns = [];
      await this.#write(accounts);
    });
    return (await this.list()).find((item) => item.id === id);
  }

  /** Brings back the login a host session refreshed: one that is newer
   * than the account's own, of the same ChatGPT account, replaces it. The
   * hosts are asked outside the lock, one call per host. A host that does
   * not answer is asked again at the next start; a live session is kept for
   * as long as it is seen live. */
  async #collect(id) {
    const pending = (await this.#account(id)).returns || [];
    if (!pending.length || !this.remoteExec) return;
    const seen = new Map();
    // ponytail: an unreachable host costs its ssh timeout once per start.
    for (const endpoint of new Set(pending.map((item) => item.endpoint))) {
      const tokens = pending
        .filter((item) => item.endpoint === endpoint)
        .map((item) => item.token);
      const out = await this.remoteExec(endpoint, CODEX_COLLECT(tokens)).catch(
        () => "",
      );
      for (const block of String(out).split(/^@@ /m).slice(1)) {
        const [token, status, ...rest] = block.split("\n");
        seen.set(token, { status, text: rest.join("\n").trim() });
      }
    }
    await this.#locked(async () => {
      const accounts = await this.#read();
      if (!Object.hasOwn(accounts, id)) return;
      const current = await this.#auth(id);
      let best = current;
      for (const { text } of seen.values())
        if (newerLogin(text, best)) best = text;
      if (best !== current) {
        const file = path.join(this.homeFor(id), "auth.json");
        await fs.writeFile(`${file}.next`, best, { mode: 0o600 });
        await fs.rename(`${file}.next`, file);
      }
      accounts[id].returns = (accounts[id].returns || []).flatMap((item) => {
        const answer = seen.get(item.token);
        if (!answer) return Date.now() - item.at > RETURN_DAYS ? [] : [item];
        return answer.status === "live" ? [{ ...item, at: Date.now() }] : [];
      });
      await this.#write(accounts);
    });
  }

  /** The account's home ready for a session (the rest of `~/.codex` linked
   * in) and its login, for a host that runs it from a copy. A ChatGPT login
   * sent to `endpoint` also gets a return token (`ret`), so a refresh made
   * there comes back. */
  async resolve(id, endpoint) {
    await this.#collect(id);
    return this.#locked(() => this.#resolve(id, endpoint));
  }
  async #resolve(id, endpoint) {
    const account = await this.#account(id);
    const auth = (await this.#auth(id)).trim();
    if (!signedInAs(auth))
      throw new Error(`Sign in to ${account.label} first.`);
    const home = this.homeFor(id);
    // What Codex would otherwise make in the account home on a first run, so
    // history and sessions stay shared.
    await fs.mkdir(path.join(this.codexHome, "sessions"), { recursive: true });
    await (
      await fs.open(path.join(this.codexHome, "history.jsonl"), "a")
    ).close();
    const shared = await fs.readdir(this.codexHome);
    for (const name of shared) {
      if (name === "auth.json") continue;
      await fs
        .symlink(path.join(this.codexHome, name), path.join(home, name))
        .catch((error) => {
          if (error.code !== "EEXIST") throw error;
        });
    }
    if (!endpoint || signedInAs(auth).mode !== "chatgpt") return { home, auth };
    const ret = randomUUID();
    const accounts = await this.#read();
    accounts[id].returns = [
      ...(accounts[id].returns || []),
      { endpoint, token: ret, at: Date.now() },
    ];
    await this.#write(accounts);
    return { home, auth, ret };
  }
}

module.exports = { CodexAccounts, signedInAs, newerLogin };
