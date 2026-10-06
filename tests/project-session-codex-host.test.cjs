const test = require("node:test");
const assert = require("node:assert/strict");
const { prepareRemoteCodexHome } = require("../electron/project-session.cjs");

const login = (accountId, at, extra = {}) =>
  JSON.stringify({
    tokens: { account_id: accountId, refresh_token: "r" },
    last_refresh: at,
    ...extra,
  });
const key = (value) => JSON.stringify({ OPENAI_API_KEY: value });

// The first exec reads the host copy; the second prepares the home and gets
// the Mac login on stdin only when it is to be written.
async function run(mac, host) {
  const calls = [];
  const exec = async (endpoint, command, options) => {
    calls.push({ command, input: options.input });
    return calls.length === 1 ? (host ?? "") : "/home/u/.sushiai/codex/acc1";
  };
  await prepareRemoteCodexHome({
    exec,
    endpoint: "ssh:h",
    accountId: "acc1",
    auth: mac,
  });
  return calls[1];
}

test("the Mac login is written when the host has none", async () => {
  const call = await run(login("a", "2026-01-02T00:00:00Z"), "");
  assert.match(call.command, / 1 '?'?$/);
  assert.ok(call.input);
});

test("the Mac login is written when it is newer for the same account", async () => {
  const call = await run(
    login("a", "2026-01-03T00:00:00Z"),
    login("a", "2026-01-02T00:00:00Z"),
  );
  assert.ok(call.input);
});

test("the Mac login is written when the host holds another account", async () => {
  const call = await run(
    login("a", "2026-01-01T00:00:00Z"),
    login("b", "2026-02-01T00:00:00Z"),
  );
  assert.ok(call.input);
});

test("the Mac login is written when the API key differs", async () => {
  const call = await run(key("sk-new"), key("sk-old"));
  assert.ok(call.input);
});

test("the Mac login replaces a host login that cannot be read", async () => {
  const call = await run(login("a", "2026-01-01T00:00:00Z"), "not json");
  assert.ok(call.input);
});

test("the host copy stays when it is a newer login of the same account", async () => {
  const call = await run(
    login("a", "2026-01-01T00:00:00Z"),
    login("a", "2026-02-01T00:00:00Z"),
  );
  assert.equal(call.input, "");
});

test("the host copy stays when the same API key is already there", async () => {
  const call = await run(key("sk-same"), key("sk-same"));
  assert.equal(call.input, "");
});
