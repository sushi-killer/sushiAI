const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { readStore } = require("../electron/app-db.cjs");
const { Projects } = require("../electron/projects.cjs");
const { registerProjectIpc } = require("../electron/ipc/projects.cjs");

function fakeSafeStorage(overrides = {}) {
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => Buffer.from(`enc:${value}`),
    decryptString: (value) => value.toString().replace(/^enc:/, ""),
    ...overrides,
  };
}

async function fixture(t, safeStorage = fakeSafeStorage()) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-store-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  return { dir, projects: new Projects({ userDataDir: dir, safeStorage }) };
}

test("resolution normalizes SSH and HTTPS remotes and ignores host identity", async (t) => {
  const { projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    git: { url: "git@github.com:team/demo.git" },
  });
  assert.equal(
    (await projects.resolve("https://github.com/team/demo"))?.id,
    project.id,
  );
  assert.equal(
    (await projects.resolve("ssh://git@github.com/team/demo.git"))?.id,
    project.id,
  );
  assert.equal(await projects.resolve(""), null);
  const local = await projects.resolve({
    remote: "https://github.com/team/demo",
    endpoint: "local",
  });
  const ssh = await projects.resolve({
    remote: "git@github.com:team/demo.git",
    endpoint: "ssh:user@devbox",
  });
  assert.equal(local.id, ssh.id);
});

test("secrets round-trip internally and IPC exposes only a masked hint", async (t) => {
  const { dir, projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "TOKEN", secret: true }],
  });
  const handlers = new Map();
  registerProjectIpc({
    handle: (channel, callback) => handlers.set(channel, callback),
    projects,
    getConnections: () => ({}),
    getPreview: () => ({}),
    terminals: new Map(),
    terminalPending: new Map(),
  });
  const result = await handlers.get("projects:secret:set")(
    project.id,
    "TOKEN",
    "invented-secret-value",
  );
  assert.deepEqual(result, { hasValue: true, hint: "••••alue" });
  assert.equal(
    await projects.secretFor(project.id, "TOKEN"),
    "invented-secret-value",
  );
  const exposed = await handlers.get("projects:get")(project.id);
  assert.equal(
    JSON.stringify(exposed).includes("invented-secret-value"),
    false,
  );
  assert.deepEqual(exposed.env[0], {
    name: "TOKEN",
    secret: true,
    hasValue: true,
    hint: "••••alue",
  });
  assert.equal(
    JSON.stringify(readStore(dir, "project-secrets")).includes(
      "invented-secret-value",
    ),
    false,
  );
});

test("unavailable secure storage fails without writing a value", async (t) => {
  const { dir, projects } = await fixture(
    t,
    fakeSafeStorage({ isEncryptionAvailable: () => false }),
  );
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "TOKEN", secret: true }],
  });
  await assert.rejects(
    projects.setSecret(project.id, "TOKEN", "invented-secret"),
    /Secure storage is unavailable/,
  );
  assert.deepEqual({ ...readStore(dir, "project-secrets") }, {});
});

test("stage environments keep setup-only and MCP-only values out of agents", async (t) => {
  const { projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [
      { name: "AGENT_TOKEN", availableTo: ["agent"] },
      { name: "INSTALL_TOKEN", availableTo: ["setup"] },
      { name: "MCP_TOKEN", availableTo: ["mcp"] },
    ],
  });
  await projects.setSecret(project.id, "AGENT_TOKEN", "agent-value");
  await projects.setSecret(project.id, "INSTALL_TOKEN", "install-value");
  await projects.setSecret(project.id, "MCP_TOKEN", "mcp-value");
  assert.deepEqual(await projects.environmentFor(project.id, "agent"), {
    AGENT_TOKEN: "agent-value",
  });
  assert.deepEqual(await projects.environmentFor(project.id, "setup"), {
    INSTALL_TOKEN: "install-value",
  });
  assert.deepEqual(await projects.agentEnvironments(), {
    [project.id]: { AGENT_TOKEN: "agent-value" },
  });
});

test("clearing or removing a secret env entry removes its ciphertext", async (t) => {
  const { dir, projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "TOKEN", secret: true }],
  });
  await projects.setSecret(project.id, "TOKEN", "invented-secret-value");
  await projects.clearSecret(project.id, "TOKEN");
  assert.equal(await projects.secretFor(project.id, "TOKEN"), null);
  await projects.setSecret(project.id, "TOKEN", "invented-secret-value");
  await projects.updateEnv(project.id, { remove: ["TOKEN"] });
  assert.equal(await projects.secretFor(project.id, "TOKEN"), null);
  const disk = JSON.stringify(readStore(dir, "project-secrets"));
  assert.equal(disk.includes("invented-secret-value"), false);
});

test("import review compares stored values without returning existing values", async (t) => {
  const { projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [
      { name: "TOKEN", secret: true },
      { name: "OTHER", secret: false },
      { name: "EMPTY", secret: false },
    ],
  });
  await projects.setSecret(project.id, "TOKEN", "invented-secret");
  await projects.setSecret(project.id, "OTHER", "plain-value");
  assert.deepEqual(
    await projects.reviewEnvImport(project.id, [
      { name: "TOKEN", value: "invented-secret" },
      { name: "OTHER", value: "new-value" },
      { name: "NEW", value: "anything" },
      { name: "EMPTY", value: "unknown" },
    ]),
    [
      { name: "TOKEN", status: "same" },
      { name: "OTHER", status: "differs" },
      { name: "NEW", status: "new" },
      { name: "EMPTY", status: "exists" },
    ],
  );
});

test("project environment values preserve whitespace and empty strings", async (t) => {
  const { projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [
      { name: "QUOTED", secret: false },
      { name: "EMPTY", secret: false },
    ],
  });
  await projects.setSecret(project.id, "QUOTED", "  value  ");
  await projects.setSecret(project.id, "EMPTY", "");
  assert.equal(await projects.secretFor(project.id, "QUOTED"), "  value  ");
  assert.equal(await projects.secretFor(project.id, "EMPTY"), "");
});

test("host environment overrides preserve the shared value and stay encrypted", async (t) => {
  const { dir, projects } = await fixture(t);
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "API_TOKEN", secret: true, hosts: ["ssh:user@devbox"] }],
  });
  await projects.setSecret(project.id, "API_TOKEN", "shared-invented-secret");
  await projects.setHostSecret(
    project.id,
    "API_TOKEN",
    "ssh:user@devbox",
    "host-invented-secret",
  );
  await projects.setHostWithheld(project.id, "ssh:user@devbox", false);
  assert.equal(
    await projects.secretFor(project.id, "API_TOKEN"),
    "shared-invented-secret",
  );
  assert.equal(
    await projects.secretForHost(project.id, "API_TOKEN", "ssh:user@devbox"),
    "host-invented-secret",
  );
  assert.equal(
    await projects.secretForHost(project.id, "API_TOKEN", "local"),
    "shared-invented-secret",
  );
  const disk = JSON.stringify(readStore(dir, "project-secrets"));
  assert.equal(disk.includes("host-invented-secret"), false);
});

test("deleting a project removes its secrets and keeps another project's", async (t) => {
  const { dir, projects } = await fixture(t);
  const env = [{ name: "TOKEN", secret: true }];
  const gone = await projects.upsert({ name: "Gone", env });
  const kept = await projects.upsert({ name: "Kept", env });
  await projects.setSecret(gone.id, "TOKEN", "invented-gone-value");
  await projects.setSecret(kept.id, "TOKEN", "invented-kept-value");
  await projects.delete(gone.id);
  assert.deepEqual(Object.keys(readStore(dir, "project-secrets")), [
    `${kept.id}:TOKEN`,
  ]);
  assert.equal(
    await projects.secretFor(kept.id, "TOKEN"),
    "invented-kept-value",
  );
});
