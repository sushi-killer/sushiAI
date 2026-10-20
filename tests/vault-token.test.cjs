const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { closeAppDb, writeStore } = require("../electron/app-db.cjs");
const { registerVault, STORE } = require("../electron/vault.cjs");
const { VAULT_MANIFEST } = require("../electron/extensions/builtin-vault.cjs");
const { createModuleIpc } = require("../electron/extensions/module-ipc.cjs");
const { tokenStateText } = require("../src/vault/tokenState.ts");

// Synthetic only: nothing here is a real token.
const TOKEN = "hvs.synthetic-lifecycle-token-52c8";
const SECOND = 1000;
const HOUR = 3600 * SECOND;
const DAY = 24 * HOUR;
const START = Date.parse("2030-01-01T00:00:00Z");

/** A clock and a timer the test moves by hand: no sleeping. `fire` jumps to
 * the timer, runs it and waits for the work it started. */
function fakeClock() {
  const clock = {
    time: START,
    timers: new Map(),
    next: 0,
    now: () => clock.time,
    setTimer(callback, ms) {
      clock.next += 1;
      const handle = { id: clock.next, ms, callback };
      clock.timers.set(handle.id, handle);
      return handle;
    },
    clearTimer(handle) {
      clock.timers.delete(handle.id);
    },
    pending: () => [...clock.timers.values()],
    async fire() {
      const [handle] = clock.pending();
      clock.timers.delete(handle.id);
      clock.time += handle.ms;
      await handle.callback();
    },
  };
  return clock;
}

/** A Vault with lookup-self and renew-self. `state.ttl` is the seconds left;
 * a renewal sets it to `renewTtl`. Every request is recorded. */
async function fakeVault(t, initial) {
  const state = {
    ttl: 3600,
    renewable: true,
    renewTtl: 3600,
    maxTtl: 0,
    period: 0,
    created: START / 1000,
    expireTime: true,
    renewStatus: 200,
    lookupStatus: 200,
    requests: [],
    ...initial,
  };
  const server = http.createServer((request, response) => {
    state.requests.push(`${request.method} ${request.url}`);
    const reply = (status, body) => {
      response.writeHead(status, { "content-type": "application/json" });
      response.end(JSON.stringify(body ?? { errors: [TOKEN] }));
    };
    if (request.headers["x-vault-token"] !== TOKEN) return reply(403);
    if (request.url === "/v1/auth/token/lookup-self") {
      if (state.lookupStatus !== 200) return reply(state.lookupStatus);
      return reply(200, {
        data: {
          ttl: state.ttl,
          renewable: state.renewable,
          explicit_max_ttl: state.maxTtl,
          period: state.period,
          creation_time: state.created,
          expire_time: state.expireTime
            ? new Date(START + state.ttl * SECOND).toISOString()
            : null,
          id: TOKEN,
        },
      });
    }
    if (request.url === "/v1/auth/token/renew-self") {
      if (state.renewStatus !== 200) return reply(state.renewStatus);
      state.ttl = state.renewTtl;
      return reply(200, { auth: { client_token: TOKEN } });
    }
    reply(404);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(
    () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  );
  state.address = `http://127.0.0.1:${server.address().port}`;
  return state;
}

const flip = (buffer) => Buffer.from(buffer.map((byte) => byte ^ 0xa5));
const safeStorage = {
  isEncryptionAvailable: () => true,
  getSelectedStorageBackend: () => "keychain",
  encryptString: (value) => flip(Buffer.from(`fake:${value}`)),
  decryptString: (buffer) => {
    const text = flip(buffer).toString();
    if (!text.startsWith("fake:")) throw new Error("bad ciphertext");
    return text.slice(5);
  },
};

/** Vault registered on a stub project store, with the test clock. */
async function setup(t, vault) {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "vault-token-test-"));
  t.after(async () => {
    closeAppDb(home);
    await fs.rm(home, { recursive: true, force: true });
  });
  const handlers = new Map();
  const notices = {
    published: [],
    retracted: [],
    register() {},
    publish(source, notice) {
      this.published.push(notice);
    },
    retract(source, key) {
      this.retracted.push(key);
    },
  };
  const clock = fakeClock();
  const instance = registerVault({
    moduleIpc: createModuleIpc({
      handle: (channel, callback) => handlers.set(channel, callback),
    }),
    projects: {
      retireField: async () => {},
      list: async () => [],
      addEnvironmentProvider() {},
      environmentChanged() {},
    },
    notices,
    safeStorage,
    userDataDir: home,
    isEnabled: () => true,
    now: clock.now,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
  });
  await instance.ready;
  const ipc = (method, ...args) =>
    handlers.get("module:invoke")(VAULT_MANIFEST.id, method, ...args);
  if (vault) await ipc("address.set", vault.address);
  return { home, clock, notices, ipc, instance };
}

const dateOf = (ms) =>
  `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;
const lastNotice = (notices) => notices.published.at(-1);

test("a renewable token is renewed at half of its time left, again and again", async (t) => {
  const vault = await fakeVault(t, { ttl: 4 * 3600, renewTtl: 4 * 3600 });
  const { clock, ipc } = await setup(t, vault);
  const status = await ipc("token.set", TOKEN);
  assert.deepEqual(status, {
    saved: true,
    state: "valid",
    expiresAt: START + 4 * HOUR,
    autoRenews: true,
  });
  assert.deepEqual(vault.requests, ["GET /v1/auth/token/lookup-self"]);
  assert.deepEqual(
    clock.pending().map((timer) => timer.ms),
    [2 * HOUR],
  );
  for (let round = 1; round <= 3; round += 1) {
    vault.ttl = 2 * 3600;
    await clock.fire();
    assert.deepEqual(vault.requests.slice(-2), [
      "POST /v1/auth/token/renew-self",
      "GET /v1/auth/token/lookup-self",
    ]);
    assert.equal(clock.pending().length, 1, "one timer, rescheduled");
    assert.equal(clock.pending()[0].ms, 2 * HOUR);
  }
  assert.equal((await ipc("token.status")).autoRenews, true);
});

test("a short ttl is renewed no sooner than the floor", async (t) => {
  const vault = await fakeVault(t, { ttl: 20, renewTtl: 20 });
  const { clock, ipc } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  assert.equal(clock.pending()[0].ms, 60 * SECOND);
});

test("a token that never expires is looked up once and needs no timer", async (t) => {
  const vault = await fakeVault(t, {
    ttl: 0,
    renewable: false,
    expireTime: false,
  });
  const { clock, notices, ipc } = await setup(t, vault);
  const status = await ipc("token.set", TOKEN);
  assert.equal(status.state, "valid");
  assert.equal(status.expiresAt, null);
  assert.equal(clock.pending().length, 0);
  assert.deepEqual(notices.published, []);
});

test("a periodic token keeps renewing with no max ttl in the way", async (t) => {
  const vault = await fakeVault(t, {
    ttl: 600,
    period: 600,
    maxTtl: 600,
    created: START / 1000 - 3600,
  });
  const { ipc } = await setup(t, vault);
  assert.equal((await ipc("token.set", TOKEN)).autoRenews, true);
});

test("a 403 on renewal is a dead token: expired state and a notice, no more timer", async (t) => {
  const vault = await fakeVault(t, { ttl: 4 * 3600 });
  const { clock, notices, ipc } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  vault.renewStatus = 403;
  await clock.fire();
  assert.equal((await ipc("token.status")).state, "expired");
  assert.equal(clock.pending().length, 0);
  assert.equal(lastNotice(notices).kind, "failed");
  assert.match(lastNotice(notices).body, /expired.*Settings → Vault/);
});

test("a 403 on the first lookup is an expired token with a notice", async (t) => {
  const vault = await fakeVault(t, {});
  const { notices, ipc } = await setup(t, vault);
  const status = await ipc("token.set", "hvs.synthetic-other-token");
  assert.equal(status.state, "expired");
  assert.equal(lastNotice(notices).key, "vault:token");
});

test("a network failure on renewal retries with growing backoff and recovers", async (t) => {
  const vault = await fakeVault(t, { ttl: 4 * 3600 });
  const { clock, notices, ipc } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  vault.renewStatus = 503;
  await clock.fire();
  assert.equal((await ipc("token.status")).state, "unreachable");
  assert.deepEqual(
    clock.pending().map((timer) => timer.ms),
    [30 * SECOND],
  );
  await clock.fire();
  assert.deepEqual(
    clock.pending().map((timer) => timer.ms),
    [60 * SECOND],
  );
  vault.renewStatus = 200;
  await clock.fire();
  const status = await ipc("token.status");
  assert.equal(status.state, "valid");
  assert.equal(status.autoRenews, true);
  assert.deepEqual(notices.published, [], "a flaky network is not a notice");
  assert.equal(clock.pending().length, 1);
});

test("a token that cannot be renewed is announced a day ahead, then reported dead", async (t) => {
  const vault = await fakeVault(t, { ttl: 7 * 86400, renewable: false });
  const { clock, notices, ipc } = await setup(t, vault);
  const status = await ipc("token.set", TOKEN);
  assert.equal(status.autoRenews, false);
  assert.deepEqual(notices.published, []);
  assert.deepEqual(
    clock.pending().map((timer) => timer.ms),
    [6 * DAY],
  );
  vault.ttl = 86400;
  await clock.fire();
  assert.equal(notices.published.length, 1);
  assert.equal(lastNotice(notices).kind, "info");
  assert.equal(
    lastNotice(notices).body,
    `Vault token expires ${dateOf(clock.time + DAY)}; save a new one in Settings → Vault.`,
  );
  assert.deepEqual(
    clock.pending().map((timer) => timer.ms),
    [DAY],
  );
  assert.equal(
    vault.requests.some((line) => line.startsWith("POST")),
    false,
    "a non-renewable token is never renewed",
  );
  vault.lookupStatus = 403;
  await clock.fire();
  assert.equal((await ipc("token.status")).state, "expired");
  assert.equal(lastNotice(notices).kind, "failed");
  assert.equal(clock.pending().length, 0);
});

test("a token already inside the last day gets the notice at once", async (t) => {
  const vault = await fakeVault(t, { ttl: 3600, renewable: false });
  const { notices, ipc } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  assert.equal(lastNotice(notices).kind, "info");
});

test("a renewable token at its max ttl is treated as not renewable", async (t) => {
  const vault = await fakeVault(t, {
    ttl: 3 * 3600,
    renewable: true,
    maxTtl: 30 * 86400,
    created: START / 1000 - 30 * 86400 + 3 * 3600,
  });
  const { notices, ipc } = await setup(t, vault);
  const status = await ipc("token.set", TOKEN);
  assert.equal(status.autoRenews, false);
  assert.equal(lastNotice(notices).kind, "info");
});

test("a token with a long wait is split so the timer never overflows", async (t) => {
  const vault = await fakeVault(t, {
    ttl: 90 * 86400,
    renewTtl: 90 * 86400,
  });
  const { clock, ipc } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  assert.equal(clock.pending()[0].ms, 2 ** 31 - 1);
  vault.ttl = 60 * 86400;
  await clock.fire();
  assert.equal(
    vault.requests.some((line) => line.startsWith("POST")),
    false,
    "the early wake only looks again",
  );
});

test("clear and stop end the timer, clear retracts the notice", async (t) => {
  const vault = await fakeVault(t, { ttl: 3600, renewable: false });
  const { clock, notices, ipc, instance } = await setup(t, vault);
  await ipc("token.set", TOKEN);
  assert.equal(clock.pending().length, 1);
  assert.deepEqual(await ipc("token.clear"), {
    saved: false,
    state: "none",
    expiresAt: null,
    autoRenews: false,
  });
  assert.equal(clock.pending().length, 0);
  assert.deepEqual(notices.retracted, ["vault:token"]);
  await ipc("token.set", TOKEN);
  assert.equal(clock.pending().length, 1);
  instance.stop();
  assert.equal(clock.pending().length, 0);
});

test("saving a new token replaces the state of the old one", async (t) => {
  const vault = await fakeVault(t, {});
  const { clock, notices, ipc } = await setup(t, vault);
  await ipc("token.set", "hvs.synthetic-other-token");
  assert.equal(lastNotice(notices).kind, "failed");
  const status = await ipc("token.set", TOKEN);
  assert.equal(status.state, "valid");
  assert.deepEqual(notices.retracted, ["vault:token"]);
  assert.equal(clock.pending().length, 1);
});

test("a token that does not decrypt is its own state with a notice", async (t) => {
  const vault = await fakeVault(t, {});
  const { home, notices, ipc, instance } = await setup(t, vault);
  writeStore(home, STORE, { token: { v: 1, ct: "bm90LWZha2U=" } });
  assert.deepEqual(await ipc("token.status"), {
    saved: true,
    state: "unreadable",
    expiresAt: null,
    autoRenews: false,
  });
  // The next start looks at it again and says so.
  const second = await setup(t, vault);
  writeStore(second.home, STORE, { token: { v: 1, ct: "bm90LWZha2U=" } });
  await second.ipc("address.set", vault.address);
  assert.equal(lastNotice(second.notices).kind, "failed");
  assert.match(lastNotice(second.notices).body, /cannot be read/);
  assert.deepEqual(notices.published, []);
  assert.equal(vault.requests.length, 0, "no request without a token");
  instance.stop();
});

test("the token and reply bodies reach no notice and no status", async (t) => {
  const vault = await fakeVault(t, { ttl: 7 * 86400, renewable: false });
  const { notices, ipc } = await setup(t, vault);
  const replies = [await ipc("token.set", TOKEN)];
  vault.lookupStatus = 403;
  replies.push(await ipc("token.set", TOKEN));
  const text = JSON.stringify([notices.published, replies]);
  assert.equal(text.includes(TOKEN), false);
});

test("the Settings line says valid until, auto-renews, expired or unreadable", () => {
  const base = { saved: true, expiresAt: null, autoRenews: false };
  const line = (patch) => tokenStateText({ ...base, ...patch });
  assert.equal(line({ state: "unchecked" }), "Saved");
  assert.equal(line({ state: "valid" }), "Saved, never expires");
  assert.match(
    line({ state: "valid", expiresAt: START, autoRenews: true }),
    /^Renews automatically, valid until /,
  );
  assert.match(
    line({ state: "valid", expiresAt: START }),
    /^Valid until .*does not renew$/,
  );
  assert.match(line({ state: "expired" }), /^Expired/);
  assert.match(line({ state: "unreadable" }), /^Cannot be read/);
  assert.match(line({ state: "unreachable" }), /unreachable/);
});
