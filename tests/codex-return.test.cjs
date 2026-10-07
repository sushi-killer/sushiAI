// A ChatGPT login is refreshed with a single-use token, so a host session
// that refreshed it hands the new one back. A host session is simulated by the
// files it leaves under ~/.sushiai/codex-sessions on a fake host; the login is
// collected through the real account store and the real collect script.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { readStore, writeStore } = require("../electron/app-db.cjs");
const { CodexAccounts, newerLogin } = require("../electron/codex-accounts.cjs");
const { makeHost } = require("./helpers/fake-host.cjs");

const login = (refresh, at, account = "acc-1") =>
  JSON.stringify({
    auth_mode: "chatgpt",
    tokens: {
      id_token: `h.${Buffer.from('{"email":"dev@example.test"}').toString("base64url")}.s`,
      refresh_token: refresh,
      account_id: account,
    },
    last_refresh: at,
  });
const FIRST = login("r1", "2026-01-01T00:00:00Z");
const REFRESHED = login("r2", "2026-01-09T00:00:00Z");

const during = {};

async function rig(t) {
  during.hook = null;
  const host = await makeHost(t, {
    bin: { ln: '#!/bin/sh\nexec /bin/ln "$@"\n' },
  });
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "codex-return-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  await fs.mkdir(path.join(dir, "dot-codex"));
  const accounts = new CodexAccounts({
    userDataDir: dir,
    codexBinary: () => null,
    codexHome: path.join(dir, "dot-codex"),
    remoteExec: async (endpoint, command) => {
      const out = await host.connections.exec(endpoint, command);
      await during.hook?.();
      return out;
    },
  });
  const { id } = await accounts.add("Work");
  await fs.writeFile(path.join(accounts.homeFor(id), "auth.json"), FIRST);
  const sessions = path.join(host.home, ".sushiai", "codex-sessions");
  // One session started on the host: the app hands out a return token, and
  // the session leaves `files` in the directory of that token.
  const run = async (files = {}) => {
    const { ret } = await accounts.resolve(id, host.endpoint);
    if (!Object.keys(files).length) return ret;
    const home = path.join(sessions, ret);
    await fs.mkdir(home, { recursive: true });
    for (const [name, text] of Object.entries(files))
      await fs.writeFile(path.join(home, name), text);
    return ret;
  };
  const own = () =>
    fs.readFile(path.join(accounts.homeFor(id), "auth.json"), "utf8");
  return { accounts, id, run, sessions, own, host, dir };
}

test("a login refreshed in a host session comes back at the next start", async (t) => {
  const { accounts, id, run, sessions, own } = await rig(t);
  await run({ ".ret": "", "auth.json": REFRESHED });
  assert.equal(await own(), FIRST);
  await accounts.resolve(id);
  assert.equal(await own(), REFRESHED);
  assert.deepEqual(await fs.readdir(sessions).catch(() => []), []);
});

test("a session that did not refresh leaves nothing on the host", async (t) => {
  const { accounts, id, run, sessions, own } = await rig(t);
  await run();
  await accounts.resolve(id);
  assert.deepEqual(await fs.readdir(sessions).catch(() => []), []);
  assert.equal(await own(), FIRST);
});

test("only a later login of the same account replaces the one kept here", () => {
  assert.equal(newerLogin(REFRESHED, FIRST), true);
  assert.equal(newerLogin(FIRST, REFRESHED), false);
  assert.equal(
    newerLogin(login("r3", "2026-02-01T00:00:00Z", "acc-2"), FIRST),
    false,
  );
  assert.equal(newerLogin("not json", FIRST), false);
});

test("a live session is read and left, a finished one is collected and removed", async (t) => {
  const { accounts, id, run, sessions, own } = await rig(t);
  const token = await run({ "auth.json": REFRESHED, pid: String(process.pid) });
  // Still running: its login is taken, its home stays.
  await accounts.resolve(id);
  assert.equal(await own(), REFRESHED);
  assert.deepEqual(await fs.readdir(sessions), [token]);
  // Its process is gone: collected and removed.
  await fs.writeFile(path.join(sessions, token, "pid"), "999999");
  await accounts.resolve(id);
  assert.deepEqual(await fs.readdir(sessions), []);
});

test("another host's session in a shared home is never judged dead here", async (t) => {
  const { sessions, host } = await rig(t);
  const other = path.join(sessions, "00000000-0000-4000-8000-000000000001");
  await fs.mkdir(other, { recursive: true });
  for (const [name, text] of [
    ["host", "another-node"],
    ["pid", "999999"],
    ["auth.json", FIRST],
    [".sent", FIRST],
  ])
    await fs.writeFile(path.join(other, name), text);
  const { CODEX_COLLECT } = require("../electron/project-session.cjs");
  const out = await host.connections.exec(
    host.endpoint,
    CODEX_COLLECT([path.basename(other)]),
  );
  assert.match(out, /^@@ \S+\nlive\n/);
  assert.deepEqual(await fs.readdir(sessions), [path.basename(other)]);
});

test("a login refreshed here while hosts answer is never replaced by an older one", async (t) => {
  const { accounts, id, run, own } = await rig(t);
  await run({ ".ret": "", "auth.json": REFRESHED });
  const LATER = login("r3", "2026-01-20T00:00:00Z");
  during.hook = () =>
    fs.writeFile(path.join(accounts.homeFor(id), "auth.json"), LATER);
  await accounts.resolve(id);
  assert.equal(await own(), LATER);
});

test("a session seen live is kept past the return window", async (t) => {
  const { accounts, id, run, dir } = await rig(t);
  await run({ "auth.json": REFRESHED, pid: String(process.pid) });
  const store = { ...readStore(dir, "codex-accounts") };
  store[id].returns[0].at = 0;
  writeStore(dir, "codex-accounts", store);
  await accounts.resolve(id);
  const after = readStore(dir, "codex-accounts")[id].returns;
  assert.equal(after.length, 1);
  assert.ok(after[0].at > 0);
});
