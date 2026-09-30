const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
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
    (
      await fs.readFile(path.join(dir, "project-secrets.json"), "utf8")
    ).includes("invented-secret-value"),
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
  await assert.rejects(fs.access(path.join(dir, "project-secrets.json")));
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
  await projects.upsert({ ...project, env: [] });
  assert.equal(await projects.secretFor(project.id, "TOKEN"), null);
  const disk = await fs.readFile(
    path.join(dir, "project-secrets.json"),
    "utf8",
  );
  assert.equal(disk.includes("invented-secret-value"), false);
});
