const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { appDb, closeAppDb, readStore } = require("../electron/app-db.cjs");
const { Projects } = require("../electron/projects.cjs");
const { registerVault, STORE } = require("../electron/vault.cjs");
const { sessionLaunchEnv } = require("../electron/project-session.cjs");
const { OrchestratorService } = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

// Synthetic only: nothing here is a real token or secret.
const TOKEN = "hvs.synthetic-vault-token-7f3a91";
const SYNTHETIC = "synthetic-vault-value-c41d07";
const STAGES = ["setup", "agent", "mcp"];

const SECRETS = {
  "app/base": {
    SHARED: "from-base",
    BASE_ONLY: "base-value",
    HAND: "vault-hand",
    BLANK: "vault-blank",
    NUM: 42,
    FLAG: true,
    "bad-name": "skipped",
    NESTED: { inner: "skipped" },
    NIL: null,
  },
  "app/over ride": { SHARED: "from-override", SECRET_VALUE: SYNTHETIC },
  garbled: "not json",
};
const PATHS = ["app/base", "app/over ride"];
// What the two paths give, merged in order, before any hand-entered value.
const MERGED = {
  SHARED: "from-override",
  BASE_ONLY: "base-value",
  HAND: "vault-hand",
  BLANK: "vault-blank",
  NUM: "42",
  FLAG: "true",
  SECRET_VALUE: SYNTHETIC,
};

/** A safeStorage stand-in whose ciphertext never holds the plain bytes. */
function fakeSafeStorage(overrides = {}) {
  const flip = (buffer) => Buffer.from(buffer.map((byte) => byte ^ 0xa5));
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => flip(Buffer.from(`fake:${value}`)),
    decryptString: (buffer) => {
      const text = flip(buffer).toString();
      if (!text.startsWith("fake:")) throw new Error("bad ciphertext");
      return text.slice(5);
    },
    ...overrides,
  };
}

/** A KV v2 server on 127.0.0.1 that checks the token and counts requests per
 * path. An error reply echoes the token and a value, so a leak would show. */
async function fakeVault(t) {
  const counts = new Map();
  const server = http.createServer((request, response) => {
    const match = /^\/v1\/secret\/data\/(.+)$/.exec(request.url);
    const secretPath = match ? decodeURIComponent(match[1]) : request.url;
    counts.set(secretPath, (counts.get(secretPath) || 0) + 1);
    setTimeout(() => {
      const token = request.headers["x-vault-token"];
      const fail = (status) => {
        response.writeHead(status, { "content-type": "application/json" });
        response.end(JSON.stringify({ errors: [`${token}`, SYNTHETIC] }));
      };
      if (token !== TOKEN || secretPath.startsWith("denied/")) return fail(403);
      if (secretPath.startsWith("broken/")) return fail(500);
      if (!Object.hasOwn(SECRETS, secretPath)) return fail(404);
      response.writeHead(200, { "content-type": "application/json" });
      const data = SECRETS[secretPath];
      response.end(
        typeof data === "string"
          ? data
          : JSON.stringify({ data: { data, metadata: { version: 1 } } }),
      );
    }, 20);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(
    () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  );
  return {
    address: `http://127.0.0.1:${server.address().port}`,
    count: (secretPath) => counts.get(secretPath) || 0,
    total: () => [...counts.values()].reduce((sum, value) => sum + value, 0),
  };
}

/** An address nothing listens on. */
async function closedAddress() {
  const server = http.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return `http://127.0.0.1:${port}`;
}

function fakeNotices() {
  return {
    registered: [],
    published: [],
    retracted: [],
    register(source, onAction) {
      this.registered.push({ source, onAction });
    },
    publish(source, notice) {
      this.published.push({ source, ...notice });
      return "none";
    },
    retract(source, key) {
      this.retracted.push({ source, key });
    },
  };
}

/** A real project store in a fresh home with Vault registered on it, and a
 * project "Vault Demo" attached to a folder with two hand-entered variables:
 * HAND (with a value) and BLANK (without one). */
async function setup(
  t,
  { safeStorage = fakeSafeStorage(), token = TOKEN } = {},
) {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "vault-test-"));
  const previousHome = process.env.HOME;
  process.env.HOME = home;
  t.after(async () => {
    process.env.HOME = previousHome;
    closeAppDb(home);
    await fs.rm(home, { recursive: true, force: true });
  });
  const projects = new Projects({ userDataDir: home, safeStorage });
  const handlers = new Map();
  const notices = fakeNotices();
  registerVault({
    handle: (channel, callback) => handlers.set(channel, callback),
    projects,
    notices,
    safeStorage,
    userDataDir: home,
  });
  const ipc = async (channel, ...args) => handlers.get(channel)(...args);
  const cwd = path.join(home, "work", "demo");
  await fs.mkdir(cwd, { recursive: true });
  const project = await projects.attach({
    endpoint: "local",
    cwd,
    name: "Vault Demo",
  });
  await projects.updateEnv(project.id, {
    set: [
      { name: "HAND", secret: true, availableTo: STAGES },
      { name: "BLANK", secret: false, availableTo: STAGES },
    ],
  });
  // Only the refused-storage tests pass no token, and they store nothing.
  if (token) {
    await projects.setSecret(project.id, "HAND", "hand-value");
    await ipc("vault:token:set", token);
  }
  return { home, projects, notices, ipc, handlers, project, cwd };
}

test("two paths merge in order and a hand-entered value wins, at every stage", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  await ipc("projects:vault:set", project.id, {
    address: vault.address,
    paths: PATHS,
  });
  for (const stage of STAGES)
    assert.deepEqual(
      await projects.environmentFor(project.id, stage, "local"),
      { ...MERGED, HAND: "hand-value" },
      stage,
    );
});

test("an ssh host gets no Vault value and the reader is never asked", async (t) => {
  const vault = await fakeVault(t);
  const { projects, project } = await setup(t);
  await projects.setVault(project.id, {
    address: vault.address,
    paths: PATHS,
  });
  for (const stage of STAGES)
    assert.deepEqual(
      await projects.environmentFor(project.id, stage, "ssh:x"),
      { HAND: "hand-value" },
      stage,
    );
  assert.equal(vault.total(), 0);
});

test("reads are cached and shared until the token changes", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  await projects.setVault(project.id, {
    address: vault.address,
    paths: PATHS,
  });
  const read = () => projects.environmentFor(project.id, "agent", "local");
  const expected = { ...MERGED, HAND: "hand-value" };
  const counts = () => PATHS.map(vault.count);
  for (const env of await Promise.all([read(), read(), read()]))
    assert.deepEqual(env, expected);
  assert.deepEqual(counts(), [1, 1], "concurrent calls share one read");
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [1, 1], "a repeated call is served from cache");
  await ipc("vault:token:set", TOKEN);
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [2, 2], "saving the token reads again");
  await ipc("vault:token:clear");
  assert.deepEqual(await read(), { HAND: "hand-value" });
  assert.deepEqual(counts(), [2, 2], "no token, no request");
  await ipc("vault:token:set", TOKEN);
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [3, 3], "the token is back: read again");
});

for (const [name, configure, body] of [
  [
    "a 403",
    async (vault, ipc) => {
      await ipc("vault:token:set", "hvs.synthetic-wrong-token");
      return vault.address;
    },
    "Vault: no access (403)",
  ],
  ["a closed port", async () => closedAddress(), "Vault: unreachable"],
])
  test(`${name} drops Vault values, keeps hand-entered ones and notifies once`, async (t) => {
    const vault = await fakeVault(t);
    const { projects, notices, ipc, project, cwd } = await setup(t);
    await projects.setVault(project.id, {
      address: await configure(vault, ipc),
      paths: PATHS,
    });
    // A session start, then the orchestrator's push of every project's values.
    const session = await sessionLaunchEnv(
      { projects },
      { host: "local", cwd, agent: "claude" },
    );
    assert.equal(session.project.id, project.id);
    assert.deepEqual(session.env, { HAND: "hand-value" });
    const manager = fakeDaemonManager({
      handlers: { "orch.settings.get": () => ({ routes: [] }) },
    });
    const service = new OrchestratorService({
      host: "local",
      getManager: () => manager,
      getConnections: () => ({}),
      send: () => {},
      getProjects: () => projects,
    });
    await service.refreshSecrets();
    const pushed = manager.requestsFor("orch.secrets.set");
    assert.equal(pushed.length, 1);
    assert.deepEqual(pushed[0].params.projects[project.id], {
      HAND: "hand-value",
    });
    assert.deepEqual(pushed[0].params.projectMcp[project.id], {
      HAND: "hand-value",
    });
    assert.deepEqual(notices.published, [
      {
        source: "vault",
        key: `vault:${project.id}`,
        kind: "failed",
        title: "Vault",
        header: "Vault Demo",
        body,
      },
    ]);
    for (const stage of STAGES)
      assert.deepEqual(
        await projects.environmentFor(project.id, stage, "local"),
        { HAND: "hand-value" },
      );
    assert.equal(notices.published.length, 1);
  });

test("each failure has its own reason, and a good read retracts the notice", async (t) => {
  const vault = await fakeVault(t);
  const { projects, notices, ipc, project } = await setup(t);
  const bodies = [];
  for (const [paths, body] of [
    [["app/base", "missing/x"], "Vault: path not found (404)"],
    [["broken/x"], "Vault: error (500)"],
    [["garbled"], "Vault: unreadable reply"],
  ]) {
    await projects.setVault(project.id, { address: vault.address, paths });
    assert.deepEqual(
      await projects.environmentFor(project.id, "agent", "local"),
      { HAND: "hand-value" },
    );
    assert.deepEqual(await ipc("projects:vault:check", project.id), {
      ok: false,
      reason: body,
    });
    bodies.push(body);
  }
  await ipc("vault:token:clear");
  assert.deepEqual(await ipc("projects:vault:check", project.id), {
    ok: false,
    reason: "Vault: no token saved",
  });
  await projects.environmentFor(project.id, "agent", "local");
  bodies.push("Vault: no token saved");
  assert.deepEqual(
    notices.published.map((notice) => notice.body),
    bodies,
  );
  assert.deepEqual(notices.retracted, []);
  await ipc("vault:token:set", TOKEN);
  await projects.setVault(project.id, {
    address: vault.address,
    paths: PATHS,
  });
  await projects.environmentFor(project.id, "agent", "local");
  assert.deepEqual(notices.retracted, [
    { source: "vault", key: `vault:${project.id}` },
  ]);
  assert.deepEqual(
    notices.registered.map((item) => item.source),
    ["vault"],
  );
});

test("a check reads fresh, gives names only and replaces the cached read", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  await projects.setVault(project.id, {
    address: vault.address,
    paths: PATHS,
  });
  await projects.environmentFor(project.id, "agent", "local");
  const names = Object.keys(MERGED).sort();
  assert.deepEqual(await ipc("projects:vault:check", project.id), {
    ok: true,
    names,
    count: names.length,
  });
  assert.deepEqual(PATHS.map(vault.count), [2, 2]);
  await projects.environmentFor(project.id, "agent", "local");
  assert.deepEqual(PATHS.map(vault.count), [2, 2]);
});

test("no Vault value or token reaches disk, logs, notices or IPC replies", async (t) => {
  const vault = await fakeVault(t);
  const output = [];
  const originals = {};
  for (const level of ["log", "warn", "error"]) {
    originals[level] = console[level];
    console[level] = (...args) => output.push(args.map(String).join(" "));
  }
  t.after(() => Object.assign(console, originals));
  const { home, projects, notices, ipc, project } = await setup(t);
  await ipc("vault:token:clear");
  const replies = [
    await ipc("vault:token:status"),
    await ipc("vault:token:set", TOKEN),
    await ipc("vault:token:status"),
  ];
  await ipc("projects:vault:set", project.id, {
    address: vault.address,
    paths: PATHS,
  });
  for (const stage of STAGES)
    assert.equal(
      (await projects.environmentFor(project.id, stage, "local")).SECRET_VALUE,
      SYNTHETIC,
    );
  replies.push(await ipc("projects:vault:check", project.id));
  replies.push(await projects.get(project.id));
  // A refused path whose reply echoes the token and a value.
  const leaky = await projects.upsert({ name: "Leaky" });
  await projects.setVault(leaky.id, {
    address: vault.address,
    paths: ["app/over ride", "denied/x"],
  });
  assert.deepEqual(
    await projects.environmentFor(leaky.id, "agent", "local"),
    {},
  );
  replies.push(await ipc("projects:vault:check", leaky.id));
  assert.equal(notices.published.length, 1);
  replies.push(await ipc("vault:token:clear"));
  replies.push(await ipc("vault:token:status"));
  const check = replies[3];
  assert.equal(check.ok, true);
  assert.ok(check.names.includes("SECRET_VALUE"));
  assert.deepEqual(Object.keys(check).sort(), ["count", "names", "ok"]);

  const files = [];
  const walk = async (dir) => {
    for (const entry of await fs.readdir(dir, { withFileTypes: true }))
      if (entry.isDirectory()) await walk(path.join(dir, entry.name));
      else files.push(path.join(dir, entry.name));
  };
  await walk(home);
  assert.ok(files.some((file) => file.endsWith("sushiai.db")));
  for (const file of files) {
    const bytes = await fs.readFile(file);
    for (const secret of [SYNTHETIC, TOKEN])
      assert.equal(bytes.includes(secret), false, `${secret} in ${file}`);
  }
  for (const [what, text] of [
    ["console", output.join("\n")],
    ["notices", JSON.stringify(notices.published)],
    ["replies", JSON.stringify(replies)],
  ])
    for (const secret of [SYNTHETIC, TOKEN])
      assert.equal(text.includes(secret), false, `${secret} in ${what}`);
});

test("the token store holds only safeStorage ciphertext and clear deletes it", async (t) => {
  const { home, ipc } = await setup(t, { token: null });
  assert.deepEqual(await ipc("vault:token:status"), { saved: false });
  for (const bad of ["", "   ", 42, "has space", "x".repeat(5000)])
    await assert.rejects(ipc("vault:token:set", bad), /Enter a Vault token/);
  assert.deepEqual(await ipc("vault:token:set", `  ${TOKEN}\n`), {
    saved: true,
  });
  const stored = readStore(home, STORE);
  assert.deepEqual(Object.keys(stored), ["token"]);
  assert.deepEqual(Object.keys(stored.token).sort(), ["ct", "v"]);
  assert.equal(stored.token.v, 1);
  assert.notEqual(stored.token.ct, TOKEN);
  assert.equal(Buffer.from(stored.token.ct, "base64").includes(TOKEN), false);
  assert.equal(
    fakeSafeStorage().decryptString(Buffer.from(stored.token.ct, "base64")),
    TOKEN,
  );
  assert.deepEqual(await ipc("vault:token:status"), { saved: true });
  assert.deepEqual(await ipc("vault:token:clear"), { saved: false });
  assert.equal(
    appDb(home)
      .prepare("SELECT COUNT(*) AS n FROM store WHERE name = ?")
      .get(STORE).n,
    0,
  );
  assert.deepEqual(await ipc("vault:token:status"), { saved: false });

  // A token that no longer decrypts counts as none.
  const { writeStore } = require("../electron/app-db.cjs");
  writeStore(home, STORE, { token: { v: 1, ct: "bm90LWZha2U=" } });
  assert.deepEqual(await ipc("vault:token:status"), { saved: false });
});

for (const [name, overrides] of [
  ["without safeStorage", { isEncryptionAvailable: () => false }],
  ["with basic_text", { getSelectedStorageBackend: () => "basic_text" }],
])
  test(`saving the token is refused ${name}`, async (t) => {
    const { home, ipc } = await setup(t, {
      safeStorage: fakeSafeStorage(overrides),
      token: null,
    });
    await assert.rejects(
      ipc("vault:token:set", TOKEN),
      /Secure storage is unavailable\./,
    );
    assert.deepEqual(Object.keys(readStore(home, STORE)), []);
  });

test("setVault checks its input, defaults the mount and survives upsert", async (t) => {
  const { projects, ipc, project } = await setup(t, { token: null });
  const base = { address: "http://127.0.0.1:8200", paths: ["app/one"] };
  for (const bad of [
    { ...base, address: "ftp://vault.invalid" },
    { ...base, address: "not a url" },
    { ...base, address: "http://user:pw@vault.invalid" },
    { ...base, address: "http://vault.invalid/?x=1" },
    { ...base, mount: "" },
    { ...base, mount: "   " },
    { ...base, mount: "/secret" },
    { ...base, mount: "secret/" },
    { ...base, paths: ["/abs"] },
    { ...base, paths: ["a/../b"] },
    { ...base, paths: [".."] },
    { ...base, paths: ["a?b"] },
    { ...base, paths: ["a#b"] },
    { ...base, paths: [""] },
    { ...base, paths: "app/one" },
    { ...base, paths: Array.from({ length: 11 }, (_, i) => `p${i}`) },
    "http://127.0.0.1:8200",
  ])
    await assert.rejects(
      projects.setVault(project.id, bad),
      undefined,
      JSON.stringify(bad),
    );
  await assert.rejects(projects.setVault("missing", base), /Unknown project/);
  const saved = await ipc("projects:vault:set", project.id, {
    address: " https://vault.invalid:8200/ ",
    paths: [" team/app ", "team/shared"],
  });
  const expected = {
    address: "https://vault.invalid:8200",
    mount: "secret",
    paths: ["team/app", "team/shared"],
  };
  assert.deepEqual(saved.vault, expected);
  await projects.upsert({ id: project.id, name: "Renamed" });
  const renamed = await projects.get(project.id);
  assert.equal(renamed.name, "Renamed");
  assert.deepEqual(renamed.vault, expected);
  assert.deepEqual(
    (await projects.setVault(project.id, { ...base, mount: "kv/team" })).vault,
    { ...base, mount: "kv/team" },
  );
  await projects.setVault(project.id, null);
  assert.equal((await projects.get(project.id)).vault, undefined);
});
