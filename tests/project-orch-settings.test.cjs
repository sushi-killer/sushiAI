const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { Projects } = require("../electron/projects.cjs");

const safeStorage = {
  isEncryptionAvailable: () => true,
  getSelectedStorageBackend: () => "keychain",
  encryptString: (value) => Buffer.from(value),
  decryptString: (value) => value.toString(),
};

async function store(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-orch-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  return new Projects({ userDataDir: dir, safeStorage });
}

test("setOrchSettings keeps the project default and a host's override", async (t) => {
  const projects = await store(t);
  const project = await projects.upsert({ name: "Demo" });
  await projects.setOrchSettings(project.id, "project", {
    autoAnswer: true,
    tiers: { hard: "b" },
  });
  await projects.setOrchSettings(project.id, "ssh:lab", { autoAnswer: false });
  const got = await projects.get(project.id);
  assert.deepEqual(got.orch, {
    defaults: { autoAnswer: true, tiers: { hard: "b" } },
    hosts: { "ssh:lab": { autoAnswer: false } },
  });
});

test("saving a stale copy of the project keeps its orchd settings", async (t) => {
  const projects = await store(t);
  const project = await projects.upsert({ name: "Demo" });
  await projects.setOrchSettings(project.id, "project", { parallel: 4 });
  await projects.upsert({ ...project, name: "Demo 2" });
  const got = await projects.get(project.id);
  assert.equal(got.name, "Demo 2");
  assert.deepEqual(got.orch.defaults, { parallel: 4 });
});

test("an empty patch clears that scope", async (t) => {
  const projects = await store(t);
  const project = await projects.upsert({ name: "Demo" });
  await projects.setOrchSettings(project.id, "ssh:lab", { parallel: 4 });
  await projects.setOrchSettings(project.id, "ssh:lab", {});
  const got = await projects.get(project.id);
  assert.equal(got.orch?.hosts?.["ssh:lab"], undefined);
});

test("setOrchSettings refuses bad input", async (t) => {
  const projects = await store(t);
  const project = await projects.upsert({ name: "Demo" });
  const evil = JSON.parse('{"__proto__": {"polluted": true}}');
  const bad = [
    [project.id, "project", []],
    [project.id, "elsewhere", {}],
    [project.id, "project", evil],
    ["missing", "project", {}],
  ];
  for (const args of bad)
    await assert.rejects(() => projects.setOrchSettings(...args));
  assert.equal(Object.prototype.polluted, undefined);
});
