const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const {
  appDb,
  closeAppDb,
  readStore,
  writeStore,
} = require("../electron/app-db.cjs");
const { Projects } = require("../electron/projects.cjs");
const {
  registerVault,
  STORE,
  PROJECTS_STORE,
  SETTINGS_STORE,
} = require("../electron/vault.cjs");
const { VAULT_MANIFEST } = require("../electron/extensions/builtin-vault.cjs");
const { createModuleIpc } = require("../electron/extensions/module-ipc.cjs");
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

// No address is set in these tests, so a saved token is never looked up.
const NO_TOKEN_STATUS = {
  saved: false,
  state: "none",
  expiresAt: null,
  autoRenews: false,
};
const SAVED_STATUS = { ...NO_TOKEN_STATUS, saved: true, state: "unchecked" };

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

/** The failure notices of projects; the token has its own notice. */
const projectNotices = (notices) =>
  notices.published.filter((notice) => notice.key !== "vault:token");

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
 * HAND (with a value) and BLANK (without one). `beforeRegister(projects,
 * home)` runs after the store exists and before the module is registered. */
async function setup(
  t,
  {
    safeStorage = fakeSafeStorage(),
    token = TOKEN,
    isEnabled = () => true,
    beforeRegister,
    publishFails = false,
  } = {},
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
  const moduleIpc = createModuleIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
  });
  const notices = fakeNotices();
  if (publishFails)
    notices.publish = () => {
      throw new Error("no notices");
    };
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
  await beforeRegister?.(projects, home, project);
  const { ready } = registerVault({
    moduleIpc,
    projects,
    notices,
    safeStorage,
    userDataDir: home,
    isEnabled,
  });
  await ready;
  const ipc = async (method, ...args) =>
    handlers.get("module:invoke")(VAULT_MANIFEST.id, method, ...args);
  // Only the refused-storage tests pass no token, and they store nothing.
  if (token) {
    await projects.setSecret(project.id, "HAND", "hand-value");
    await ipc("token.set", token);
  }
  return { home, projects, notices, ipc, handlers, project, cwd };
}

test("two paths merge in order and a hand-entered value wins, at every stage", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  for (const stage of STAGES)
    assert.deepEqual(
      await projects.environmentFor(project.id, stage, "local"),
      { ...MERGED, HAND: "hand-value" },
      stage,
    );
});

test("an ssh host gets no Vault value and the reader is never asked", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
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
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  const read = () => projects.environmentFor(project.id, "agent", "local");
  const expected = { ...MERGED, HAND: "hand-value" };
  const counts = () => PATHS.map(vault.count);
  for (const env of await Promise.all([read(), read(), read()]))
    assert.deepEqual(env, expected);
  assert.deepEqual(counts(), [1, 1], "concurrent calls share one read");
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [1, 1], "a repeated call is served from cache");
  await ipc("token.set", TOKEN);
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [2, 2], "saving the token reads again");
  await ipc("token.clear");
  assert.deepEqual(await read(), { HAND: "hand-value" });
  assert.deepEqual(counts(), [2, 2], "no token, no request");
  await ipc("token.set", TOKEN);
  assert.deepEqual(await read(), expected);
  assert.deepEqual(counts(), [3, 3], "the token is back: read again");
});

for (const [name, configure, body] of [
  [
    "a 403",
    async (vault, ipc) => {
      await ipc("token.set", "hvs.synthetic-wrong-token");
      return vault.address;
    },
    "Vault: no access (403)",
  ],
  ["a closed port", async () => closedAddress(), "Vault: unreachable"],
])
  test(`${name} drops Vault values, keeps hand-entered ones and notifies once`, async (t) => {
    const vault = await fakeVault(t);
    const { projects, notices, ipc, project, cwd } = await setup(t);
    await ipc("address.set", await configure(vault, ipc));
    await ipc("project.set", project.id, { paths: PATHS });
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
    assert.deepEqual(projectNotices(notices), [
      {
        source: VAULT_MANIFEST.id,
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
    assert.equal(projectNotices(notices).length, 1);
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
    await ipc("address.set", vault.address);
    await ipc("project.set", project.id, { paths });
    assert.deepEqual(
      await projects.environmentFor(project.id, "agent", "local"),
      { HAND: "hand-value" },
    );
    assert.deepEqual(await ipc("project.names", project.id), {
      ok: false,
      reason: body,
    });
    bodies.push(body);
  }
  await ipc("token.clear");
  assert.deepEqual(await ipc("project.names", project.id), {
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
  await ipc("token.set", TOKEN);
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  await projects.environmentFor(project.id, "agent", "local");
  assert.deepEqual(notices.retracted, [
    { source: VAULT_MANIFEST.id, key: `vault:${project.id}` },
  ]);
  assert.deepEqual(
    notices.registered.map((item) => item.source),
    [VAULT_MANIFEST.id],
  );
});

test("project.names gives sorted names, reuses a good read and re-reads a failed one", async (t) => {
  const vault = await fakeVault(t);
  const { projects, notices, ipc, project } = await setup(t);
  assert.equal(await ipc("project.names", project.id), null, "no paths");
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  const names = Object.keys(MERGED).sort();
  assert.deepEqual(await ipc("project.names", project.id), {
    ok: true,
    names,
  });
  assert.deepEqual(PATHS.map(vault.count), [1, 1]);
  assert.deepEqual(await ipc("project.names", project.id), { ok: true, names });
  await projects.environmentFor(project.id, "agent", "local");
  assert.deepEqual(PATHS.map(vault.count), [1, 1], "the good read is reused");

  // A failed cached read is read again, and the new read replaces it.
  await ipc("token.set", "hvs.synthetic-wrong-token");
  assert.deepEqual(await ipc("project.names", project.id), {
    ok: false,
    reason: "Vault: no access (403)",
  });
  const failed = vault.count(PATHS[0]);
  await ipc("token.set", TOKEN);
  assert.deepEqual(await ipc("project.names", project.id), { ok: true, names });
  assert.equal(vault.count(PATHS[0]), failed + 1);
  await projects.environmentFor(project.id, "agent", "local");
  assert.equal(vault.count(PATHS[0]), failed + 1, "the new read is cached");
  assert.deepEqual(projectNotices(notices), [], "names never publish a notice");
});

test("project.names retracts the failure notice once a read is good", async (t) => {
  const vault = await fakeVault(t);
  const { projects, notices, ipc, project } = await setup(t);
  await ipc("address.set", await closedAddress());
  await ipc("project.set", project.id, { paths: PATHS });
  await projects.environmentFor(project.id, "agent", "local");
  assert.equal(notices.published.length, 1);
  await ipc("address.set", vault.address);
  assert.equal((await ipc("project.names", project.id)).ok, true);
  assert.deepEqual(notices.retracted, [
    { source: VAULT_MANIFEST.id, key: `vault:${project.id}` },
  ]);
});

test("no address set: names, values and the notice say so, before the token", async (t) => {
  const { projects, notices, ipc, project } = await setup(t);
  await ipc("token.clear");
  assert.deepEqual(await ipc("address.get"), reply(""));
  await ipc("project.set", project.id, { paths: PATHS });
  assert.deepEqual(await ipc("project.names", project.id), {
    ok: false,
    reason: "Vault: no address set",
  });
  assert.deepEqual(
    await projects.environmentFor(project.id, "agent", "local"),
    { HAND: "hand-value" },
  );
  assert.deepEqual(
    notices.published.map((notice) => notice.body),
    ["Vault: no address set"],
  );
});

test("address.set checks its input, normalizes and clears; address.get returns it", async (t) => {
  const { ipc } = await setup(t, { token: null });
  for (const bad of [
    "ftp://vault.example.com",
    "not a url",
    "http://user:pw@vault.example.com",
    "http://vault.example.com/?x=1",
    "http://vault.example.com/#x",
    42,
    null,
    { address: "http://vault.example.com" },
  ])
    await assert.rejects(
      ipc("address.set", bad),
      /must be an http\(s\) URL/,
      JSON.stringify(bad),
    );
  assert.deepEqual(await ipc("address.get"), reply(""));
  assert.deepEqual(
    await ipc("address.set", " https://vault.example.com:8200/ "),
    reply("https://vault.example.com:8200"),
  );
  assert.deepEqual(
    await ipc("address.get"),
    reply("https://vault.example.com:8200"),
  );
  assert.deepEqual(await ipc("address.set", "   "), reply(""));
  assert.deepEqual(await ipc("address.get"), reply(""));
});

test("a new address drops the cache and tells the project store", async (t) => {
  const vault = await fakeVault(t);
  const { projects, ipc, project } = await setup(t);
  let changed = 0;
  const original = projects.environmentChanged.bind(projects);
  projects.environmentChanged = () => {
    changed += 1;
    return original();
  };
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  const before = changed;
  const read = () => projects.environmentFor(project.id, "agent", "local");
  assert.equal((await read()).SECRET_VALUE, SYNTHETIC);
  assert.deepEqual(PATHS.map(vault.count), [1, 1]);
  assert.deepEqual(
    await ipc("address.set", `${vault.address}/`),
    reply(vault.address),
  );
  assert.equal(changed, before + 1);
  assert.equal((await read()).SECRET_VALUE, SYNTHETIC);
  assert.deepEqual(PATHS.map(vault.count), [2, 2], "read again");
  await ipc("address.set", await closedAddress());
  assert.equal((await read()).SECRET_VALUE, undefined);
  assert.equal(changed, before + 2);
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
  await ipc("token.clear");
  const replies = [
    await ipc("token.status"),
    await ipc("token.set", TOKEN),
    await ipc("token.status"),
  ];
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  for (const stage of STAGES)
    assert.equal(
      (await projects.environmentFor(project.id, stage, "local")).SECRET_VALUE,
      SYNTHETIC,
    );
  replies.push(await ipc("project.names", project.id));
  replies.push(await ipc("address.get"));
  replies.push(await ipc("project.get", project.id));
  replies.push(await projects.get(project.id));
  // A refused path whose reply echoes the token and a value.
  const leaky = await projects.upsert({ name: "Leaky" });
  await ipc("address.set", vault.address);
  await ipc("project.set", leaky.id, { paths: ["app/over ride", "denied/x"] });
  assert.deepEqual(
    await projects.environmentFor(leaky.id, "agent", "local"),
    {},
  );
  replies.push(await ipc("project.names", leaky.id));
  assert.equal(notices.published.length, 1);
  replies.push(await ipc("token.clear"));
  replies.push(await ipc("token.status"));
  const check = replies[3];
  assert.equal(check.ok, true);
  assert.ok(check.names.includes("SECRET_VALUE"));
  assert.deepEqual(Object.keys(check).sort(), ["names", "ok"]);

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
  assert.deepEqual(await ipc("token.status"), NO_TOKEN_STATUS);
  for (const bad of ["", "   ", 42, "has space", "x".repeat(5000)])
    await assert.rejects(ipc("token.set", bad), /Enter a Vault token/);
  assert.deepEqual(await ipc("token.set", `  ${TOKEN}\n`), SAVED_STATUS);
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
  assert.deepEqual(await ipc("token.status"), SAVED_STATUS);
  assert.deepEqual(await ipc("token.clear"), NO_TOKEN_STATUS);
  assert.equal(
    appDb(home)
      .prepare("SELECT COUNT(*) AS n FROM store WHERE name = ?")
      .get(STORE).n,
    0,
  );
  assert.deepEqual(await ipc("token.status"), NO_TOKEN_STATUS);

  // A token that no longer decrypts is saved but unreadable, not "none".
  writeStore(home, STORE, { token: { v: 1, ct: "bm90LWZha2U=" } });
  assert.deepEqual(await ipc("token.status"), {
    ...SAVED_STATUS,
    state: "unreadable",
  });
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
      ipc("token.set", TOKEN),
      /Secure storage is unavailable\./,
    );
    assert.deepEqual(Object.keys(readStore(home, STORE)), []);
  });

test("project.set checks its input, defaults the mount, ignores an address and survives upsert", async (t) => {
  const { home, projects, ipc, project } = await setup(t, { token: null });
  const base = { paths: ["app/one"] };
  for (const bad of [
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
    "app/one",
  ])
    await assert.rejects(
      ipc("project.set", project.id, bad),
      undefined,
      JSON.stringify(bad),
    );
  await assert.rejects(ipc("project.set", "missing", base), /Unknown project/);
  const expected = { mount: "secret", paths: ["team/app", "team/shared"] };
  assert.deepEqual(
    await ipc("project.set", project.id, {
      address: "https://vault.example.com",
      paths: [" team/app ", "team/shared"],
    }),
    expected,
  );
  assert.deepEqual(readStore(home, PROJECTS_STORE)[project.id], expected);
  assert.deepEqual(
    await ipc("address.get"),
    reply(""),
    "a project never sets the address",
  );
  await projects.upsert({ id: project.id, name: "Renamed" });
  assert.equal((await projects.get(project.id)).name, "Renamed");
  assert.equal("vault" in (await projects.get(project.id)), false);
  assert.deepEqual(await ipc("project.get", project.id), expected);
  assert.deepEqual(
    await ipc("project.set", project.id, { ...base, mount: "kv/team" }),
    { ...base, mount: "kv/team" },
  );
  assert.equal(await ipc("project.set", project.id, null), null);
  assert.equal(await ipc("project.get", project.id), null);
});

// Per-project addresses of older versions: projects named in `spec` are made
// and their entries written into the store before the module is registered,
// so the move at start is what the test drives. A name starting with "gone:"
// has an entry but no project.
const seedNamed = (spec) => async (store, dir) => {
  const entries = {};
  for (const [name, entry] of spec) {
    const id = name.startsWith("gone:")
      ? name
      : (await store.upsert({ name })).id;
    entries[id] = entry;
  }
  writeStore(dir, PROJECTS_STORE, entries);
};
const entry = (address, paths = ["a"]) => ({
  ...(address === undefined ? {} : { address }),
  mount: "secret",
  paths,
});
const NO_ADDRESS_IN_ENTRIES = (home) =>
  Object.values(readStore(home, PROJECTS_STORE)).every(
    (item) => !("address" in item),
  );
const reply = (address, conflict = "") => ({ address, conflict });

test("migration: no per-project address leaves the global address empty and publishes nothing", async (t) => {
  const { home, notices, ipc } = await setup(t, {
    beforeRegister: seedNamed([["Solo", entry()]]),
  });
  assert.deepEqual(await ipc("address.get"), reply(""));
  assert.deepEqual(notices.published, []);
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
});

test("migration: one address becomes the global address and is stripped from projects", async (t) => {
  const { home, notices, ipc } = await setup(t, {
    beforeRegister: seedNamed([
      ["P1", entry("https://vault.example.com/", ["a"])],
      ["P2", entry("https://vault.example.com", ["b"])],
    ]),
  });
  assert.deepEqual(
    await ipc("address.get"),
    reply("https://vault.example.com"),
  );
  assert.deepEqual(
    { ...readStore(home, SETTINGS_STORE) },
    { address: "https://vault.example.com" },
  );
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
  assert.deepEqual(Object.values(readStore(home, PROJECTS_STORE))[0], {
    mount: "secret",
    paths: ["a"],
  });
  assert.deepEqual(notices.published, []);
});

test("migration: several addresses keep the most used, name the rest in one notice", async (t) => {
  const { home, notices, ipc } = await setup(t, {
    beforeRegister: seedNamed([
      ["Alpha", entry("https://one.example.com")],
      ["Beta", entry("https://two.example.com")],
      ["Gamma", entry("https://two.example.com")],
    ]),
  });
  const body =
    "Projects used 2 Vault addresses. Kept https://two.example.com (2 projects). " +
    "Not kept: https://one.example.com (Alpha). Change it in Settings, Vault tab.";
  assert.deepEqual(
    await ipc("address.get"),
    reply("https://two.example.com", body),
  );
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
  assert.deepEqual(notices.published, [
    {
      source: VAULT_MANIFEST.id,
      key: "vault:address",
      kind: "info",
      title: "Vault",
      header: "Vault address",
      body,
    },
  ]);
});

test("migration: a tie keeps the first address in store order", async (t) => {
  const { ipc, notices } = await setup(t, {
    beforeRegister: seedNamed([
      ["Alpha", entry("https://first.example.com")],
      ["Beta", entry("https://second.example.com")],
    ]),
  });
  assert.equal((await ipc("address.get")).address, "https://first.example.com");
  assert.match(
    notices.published[0].body,
    /Not kept: https:\/\/second\.example\.com \(Beta\)/,
  );
});

test("migration: a global address already set stays, and differing project addresses are named", async (t) => {
  const { home, ipc, notices } = await setup(t, {
    beforeRegister: async (store, dir) => {
      writeStore(dir, SETTINGS_STORE, {
        address: "https://global.example.com",
      });
      await seedNamed([["Alpha", entry("https://old.example.com")]])(
        store,
        dir,
      );
    },
  });
  const settings = await ipc("address.get");
  assert.equal(settings.address, "https://global.example.com");
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
  assert.equal(notices.published.length, 1);
  assert.match(
    settings.conflict,
    /Kept https:\/\/global\.example\.com \(0 projects\)/,
  );
});

test("migration runs once: a second start changes nothing and publishes nothing", async (t) => {
  const first = await setup(t, {
    beforeRegister: seedNamed([
      ["Alpha", entry("https://one.example.com")],
      ["Beta", entry("https://two.example.com")],
    ]),
  });
  assert.equal(first.notices.published.length, 1);
  const before = {
    settings: { ...readStore(first.home, SETTINGS_STORE) },
    projects: { ...readStore(first.home, PROJECTS_STORE) },
  };
  const notices = fakeNotices();
  const { ready } = registerVault({
    moduleIpc: createModuleIpc({ handle: () => {} }),
    projects: first.projects,
    notices,
    safeStorage: fakeSafeStorage(),
    userDataDir: first.home,
    isEnabled: () => true,
  });
  await ready;
  assert.deepEqual(notices.published, []);
  assert.deepEqual(
    { ...readStore(first.home, SETTINGS_STORE) },
    before.settings,
  );
  assert.deepEqual(
    { ...readStore(first.home, PROJECTS_STORE) },
    before.projects,
  );
});

test("migration: a notice that fails never breaks the move, and the conflict text survives a restart", async (t) => {
  const { home, projects, ipc } = await setup(t, {
    beforeRegister: seedNamed([
      ["Alpha", entry("https://one.example.com")],
      ["Beta", entry("https://two.example.com")],
    ]),
    publishFails: true,
  });
  const settings = await ipc("address.get");
  assert.equal(settings.address, "https://one.example.com");
  assert.match(
    settings.conflict,
    /Not kept: https:\/\/two\.example\.com \(Beta\)/,
  );
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
  // A new start on the same folder still has the text.
  const handlers = new Map();
  registerVault({
    moduleIpc: createModuleIpc({
      handle: (channel, callback) => handlers.set(channel, callback),
    }),
    projects,
    notices: fakeNotices(),
    safeStorage: fakeSafeStorage(),
    userDataDir: home,
    isEnabled: () => true,
  });
  assert.deepEqual(
    await handlers.get("module:invoke")(VAULT_MANIFEST.id, "address.get"),
    settings,
  );
});

test("address.dismiss clears the conflict and keeps the address; address.set clears it too", async (t) => {
  const make = () =>
    setup(t, {
      beforeRegister: seedNamed([
        ["Alpha", entry("https://one.example.com")],
        ["Beta", entry("https://two.example.com")],
      ]),
    });
  const a = await make();
  assert.notEqual((await a.ipc("address.get")).conflict, "");
  assert.deepEqual(
    await a.ipc("address.dismiss"),
    reply("https://one.example.com"),
  );
  assert.deepEqual(
    { ...readStore(a.home, SETTINGS_STORE) },
    { address: "https://one.example.com" },
  );
  const b = await make();
  assert.notEqual((await b.ipc("address.get")).conflict, "");
  assert.deepEqual(
    await b.ipc("address.set", "https://three.example.com"),
    reply("https://three.example.com"),
  );
  assert.deepEqual(
    await b.ipc("address.get"),
    reply("https://three.example.com"),
  );
});

test("migration: projects that are gone do not vote or appear, and their entries go", async (t) => {
  const { home, notices, ipc } = await setup(t, {
    beforeRegister: seedNamed([
      ["gone:x1", entry("https://ghost.example.com")],
      ["gone:x2", entry("https://ghost.example.com")],
      ["Alive", entry("https://real.example.com")],
    ]),
  });
  assert.deepEqual(await ipc("address.get"), reply("https://real.example.com"));
  assert.deepEqual(notices.published, []);
  const left = readStore(home, PROJECTS_STORE);
  assert.equal(Object.keys(left).length, 1);
  assert.ok(NO_ADDRESS_IN_ENTRIES(home));
});

test("a stored per-project address never beats the global one, and project.get never returns it", async (t) => {
  const vault = await fakeVault(t);
  const { home, projects, ipc, project } = await setup(t);
  await ipc("address.set", vault.address);
  // An entry written behind the module's back, as an older build left it.
  writeStore(home, PROJECTS_STORE, {
    [project.id]: {
      address: await closedAddress(),
      mount: "secret",
      paths: PATHS,
    },
  });
  assert.deepEqual(await ipc("project.get", project.id), {
    mount: "secret",
    paths: PATHS,
  });
  assert.equal(
    (await projects.environmentFor(project.id, "agent", "local")).SECRET_VALUE,
    SYNTHETIC,
  );
  assert.equal((await ipc("project.names", project.id)).ok, true);
});

test("a failed environment read, a names read, then another environment read notify once", async (t) => {
  const { projects, notices, ipc, project } = await setup(t);
  await ipc("address.set", await closedAddress());
  await ipc("project.set", project.id, { paths: PATHS });
  await projects.environmentFor(project.id, "agent", "local");
  assert.deepEqual(await ipc("project.names", project.id), {
    ok: false,
    reason: "Vault: unreachable",
  });
  await projects.environmentFor(project.id, "agent", "local");
  assert.equal(notices.published.length, 1);
});

test("a legacy project with a bad address keeps its mount and paths", async (t) => {
  let seeded;
  const { ipc, project } = await setup(t, {
    beforeRegister: async (store, dir, created) => {
      seeded = created.id;
      const db = appDb(dir);
      const row = db
        .prepare("SELECT data FROM projects WHERE id = ?")
        .get(created.id);
      db.prepare("UPDATE projects SET data = ? WHERE id = ?").run(
        JSON.stringify({
          ...JSON.parse(row.data),
          vault: { address: "not a url", mount: "kv", paths: ["team/app"] },
        }),
        created.id,
      );
    },
  });
  assert.equal(project.id, seeded);
  assert.deepEqual(await ipc("project.get", project.id), {
    mount: "kv",
    paths: ["team/app"],
  });
  assert.deepEqual(await ipc("address.get"), reply(""));
});

test("a stored project with the old vault field moves into the module, address included", async (t) => {
  const vault = await fakeVault(t);
  const old = { address: vault.address, mount: "secret", paths: PATHS };
  let projectId;
  const { home, projects, ipc, project } = await setup(t, {
    beforeRegister: async (store, dir, created) => {
      projectId = created.id;
      const db = appDb(dir);
      const row = db
        .prepare("SELECT data FROM projects WHERE id = ?")
        .get(created.id);
      db.prepare("UPDATE projects SET data = ? WHERE id = ?").run(
        JSON.stringify({ ...JSON.parse(row.data), vault: old }),
        created.id,
      );
    },
  });
  assert.equal(project.id, projectId);
  const expected = { mount: "secret", paths: PATHS };
  assert.deepEqual(await ipc("project.get", project.id), expected);
  assert.deepEqual(await ipc("address.get"), reply(vault.address));
  assert.equal("vault" in (await projects.get(project.id)), false);
  const row = appDb(home)
    .prepare("SELECT data FROM projects WHERE id = ?")
    .get(project.id);
  assert.equal("vault" in JSON.parse(row.data), false);
  assert.deepEqual(readStore(home, PROJECTS_STORE)[project.id], expected);
  assert.deepEqual(
    await projects.environmentFor(project.id, "agent", "local"),
    { ...MERGED, HAND: "hand-value" },
  );
});

test("with the module off the project keeps only hand-entered values and Vault is not asked", async (t) => {
  const vault = await fakeVault(t);
  let enabled = true;
  const { projects, ipc, project } = await setup(t, {
    isEnabled: () => enabled,
  });
  await ipc("address.set", vault.address);
  await ipc("project.set", project.id, { paths: PATHS });
  enabled = false;
  for (const stage of STAGES)
    assert.deepEqual(
      await projects.environmentFor(project.id, stage, "local"),
      { HAND: "hand-value" },
      stage,
    );
  assert.equal(vault.total(), 0);
  enabled = true;
  assert.equal(
    (await projects.environmentFor(project.id, "agent", "local")).SECRET_VALUE,
    SYNTHETIC,
  );
});
