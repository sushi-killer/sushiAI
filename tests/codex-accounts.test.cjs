// Codex accounts sign in through a real `codex login` run (a fake binary that
// writes auth.json the way Codex does) into a home of their own.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { CodexAccounts } = require("../electron/codex-accounts.cjs");

const KEY = "sk-invented-codex-key-1234";

async function rig(t, script) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "codex-accounts-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const binary = path.join(dir, "codex");
  await fs.writeFile(binary, `#!/bin/sh\n${script}\n`, { mode: 0o755 });
  const codexHome = path.join(dir, "dot-codex");
  await fs.mkdir(path.join(codexHome, "sessions"), { recursive: true });
  await fs.writeFile(path.join(codexHome, "config.toml"), "");
  await fs.writeFile(path.join(codexHome, "auth.json"), '{"tokens":"own"}');
  const accounts = new CodexAccounts({
    userDataDir: path.join(dir, "data"),
    codexBinary: () => binary,
    codexHome,
  });
  return { accounts, codexHome };
}

const API_KEY_LOGIN = `[ "$2" = --with-api-key ] || exit 3
printf '{"auth_mode":"apikey","OPENAI_API_KEY":"%s"}' "$(cat)" > "$CODEX_HOME/auth.json"`;

test("an API key signs an account in; the list says who, never the key", async (t) => {
  const { accounts } = await rig(t, API_KEY_LOGIN);
  const { id } = await accounts.add("Work");
  const signed = await accounts.login(id, `  ${KEY}\n`);
  assert.deepEqual(
    { signedIn: signed.signedIn, mode: signed.mode, detail: signed.detail },
    { signedIn: true, mode: "apiKey", detail: "…1234" },
  );
  assert.equal(JSON.stringify(await accounts.list()).includes(KEY), false);
  const auth = JSON.parse(
    await fs.readFile(path.join(accounts.homeFor(id), "auth.json"), "utf8"),
  );
  assert.equal(auth.OPENAI_API_KEY, KEY);
});

test("a browser sign-in shows the ChatGPT email from the login Codex wrote", async (t) => {
  const claims = Buffer.from(
    JSON.stringify({ email: "dev@example.test" }),
  ).toString("base64url");
  const { accounts } = await rig(
    t,
    `[ -z "$2" ] || exit 3
printf '{"auth_mode":"chatgpt","tokens":{"id_token":"h.${claims}.s"}}' > "$CODEX_HOME/auth.json"`,
  );
  const { id } = await accounts.add("Personal");
  assert.equal((await accounts.login(id)).detail, "dev@example.test");
});

test("a failed sign-in reports what Codex said and leaves the account signed out", async (t) => {
  const { accounts } = await rig(
    t,
    'echo "Error: port 1455 is in use" >&2; exit 1',
  );
  const { id } = await accounts.add("Work");
  await assert.rejects(accounts.login(id), /port 1455 is in use/);
  const [account] = await accounts.list();
  assert.equal(account.signedIn, false);
  assert.equal(account.signingIn, false);
  await assert.rejects(accounts.login(id, "  "), /Paste an OpenAI API key/);
});

test("a login that closes stdin reports its failure and can be retried", async (t) => {
  const { accounts } = await rig(
    t,
    'exec 0<&-\necho "Login refused before reading the key" >&2\nexit 1',
  );
  const { id } = await accounts.add("Work");
  await assert.rejects(
    accounts.login(id, "synthetic-key-".repeat(65536)),
    /Login refused before reading the key/,
  );
  const [failed] = await accounts.list();
  assert.equal(failed.signedIn, false);
  assert.equal(failed.signingIn, false);
  await fs.writeFile(accounts.codexBinary(), `#!/bin/sh\n${API_KEY_LOGIN}\n`, {
    mode: 0o755,
  });
  assert.equal((await accounts.login(id, KEY)).signedIn, true);
});

test("a session home shares ~/.codex but keeps its own login", async (t) => {
  const { accounts, codexHome } = await rig(t, API_KEY_LOGIN);
  const { id } = await accounts.add("Work");
  await assert.rejects(accounts.resolve(id), /Sign in to Work first/);
  await accounts.login(id, KEY);
  const { home, auth } = await accounts.resolve(id);
  assert.equal(JSON.parse(auth).OPENAI_API_KEY, KEY);
  assert.equal(
    await fs.readlink(path.join(home, "sessions")),
    path.join(codexHome, "sessions"),
  );
  assert.equal(
    (await fs.lstat(path.join(home, "auth.json"))).isSymbolicLink(),
    false,
  );
  // A second start finds its links already there.
  await accounts.resolve(id);
  assert.equal(
    await fs.readFile(path.join(codexHome, "auth.json"), "utf8"),
    '{"tokens":"own"}',
  );
});

test("removing an account removes its home; ids it never made are refused", async (t) => {
  const { accounts } = await rig(t, API_KEY_LOGIN);
  const { id } = await accounts.add("Work");
  await accounts.login(id, KEY);
  await accounts.remove(id);
  await assert.rejects(fs.stat(accounts.homeFor(id)));
  assert.deepEqual(await accounts.list(), []);
  for (const bad of ["__proto__", "../..", id, 7])
    await assert.rejects(accounts.resolve(bad), /Unknown Codex account/);
});

test("Codex session resolver tags missing sign-in but preserves damaged or unreadable auth", async (t) => {
  const { accounts } = await rig(t, API_KEY_LOGIN);
  await assert.rejects(accounts.resolve("missing"), {
    code: "ACCOUNT_NOT_CONFIGURED",
  });
  const { id } = await accounts.add("Work");
  await assert.rejects(accounts.resolve(id), {
    code: "ACCOUNT_NOT_CONFIGURED",
  });
  const auth = path.join(accounts.homeFor(id), "auth.json");
  await fs.writeFile(auth, "{broken");
  await assert.rejects(accounts.resolve(id), SyntaxError);
  await fs.rm(auth);
  await fs.mkdir(auth);
  await assert.rejects(accounts.resolve(id), { code: "EISDIR" });
});
