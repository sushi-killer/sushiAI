const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} = require("node:fs/promises");
const { tmpdir } = require("node:os");
const path = require("node:path");

const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const { HERDR_MANIFEST } = require("../electron/extensions/builtin-herdr.cjs");

const probeFixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);
const contribution = structuredClone(probeFixture.contributions);
const builtin = {
  id: "sushiai.workspace",
  name: "Workspace",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  contributions: { surfaces: [], navigation: [], actions: [], commands: [] },
};
const installed = {
  id: "user.specimen",
  name: "Specimens",
  version: "1.2.3",
  apiVersion: 1,
  source: {
    kind: "git",
    url: "https://example.test/specimen.git",
    requestedRef: "v1.2.3",
    resolvedCommit: "0123456789abcdef",
  },
  contributions: contribution,
};

test("extension manager persists enabled state, lock metadata and safe overrides", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const first = new ExtensionManager({
      dataDir: dir,
      builtins: [builtin],
      installed: [installed],
    });
    const initial = await first.list();
    assert.equal(initial.extensions.length, 2);
    assert.equal(
      initial.extensions.find((item) => item.manifest.id === "user.specimen")
        .status,
      "disabled",
    );
    assert.equal(initial.version, 0);
    await first.setEnabled("user.specimen", true);
    assert.equal((await first.list()).version, 1);
    await first.setEnabled("user.specimen", false);
    const second = new ExtensionManager({
      dataDir: dir,
      builtins: [builtin],
      installed: [installed],
    });
    const restored = await second.list();
    assert.equal(
      restored.extensions.find((item) => item.manifest.id === "user.specimen")
        .status,
      "disabled",
    );
    const lock = JSON.parse(
      await readFile(
        path.join(dir, "extensions", "extension-lock.json"),
        "utf8",
      ),
    );
    assert.equal(
      lock.packages["user.specimen"].source.resolvedCommit,
      "0123456789abcdef",
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("Herdr is a bundled extension provider and cannot be disabled as an external package", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const manager = new ExtensionManager({
      dataDir: dir,
      builtins: [HERDR_MANIFEST],
    });
    const snapshot = await manager.list();
    assert.equal(snapshot.extensions[0].manifest.id, "builtin.herdr");
    assert.equal(snapshot.extensions[0].manifest.source.kind, "builtin");
    assert.equal(snapshot.extensions[0].status, "active");
    await assert.rejects(
      () => manager.setEnabled("builtin.herdr", false),
      /cannot be disabled/,
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("corrupted external state fails closed and is not overwritten", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const manager = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    await manager.ready;
    const stateFile = path.join(dir, "extensions", "extensions.json");
    await writeFile(stateFile, "{broken", "utf8");
    const recovered = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    const snapshot = await recovered.list();
    assert.equal(snapshot.extensions[0].status, "disabled");
    assert.match(snapshot.diagnostic, /disabled/);
    assert.equal(await readFile(stateFile, "utf8"), "{broken");
    await assert.rejects(
      () => recovered.setEnabled("user.specimen", true),
      /settings are repaired/,
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("invalid lock metadata fails closed and is not overwritten", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const first = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    await first.ready;
    const lockFile = path.join(dir, "extensions", "extension-lock.json");
    await writeFile(lockFile, "{broken", "utf8");
    const recovered = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    const snapshot = await recovered.list();
    assert.equal(snapshot.extensions[0].status, "disabled");
    assert.match(snapshot.diagnostic, /lock metadata/);
    assert.equal(await readFile(lockFile, "utf8"), "{broken");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("a lock entry that no longer matches an external manifest fails closed", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const first = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    await first.ready;
    const lockFile = path.join(dir, "extensions", "extension-lock.json");
    const lock = JSON.parse(await readFile(lockFile, "utf8"));
    lock.packages["user.specimen"].version = "9.9.9";
    await writeFile(lockFile, JSON.stringify(lock), "utf8");
    const recovered = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    const snapshot = await recovered.list();
    assert.equal(snapshot.extensions[0].status, "disabled");
    assert.match(snapshot.diagnostic, /does not match/);
    assert.equal(
      JSON.parse(await readFile(lockFile, "utf8")).packages["user.specimen"]
        .version,
      "9.9.9",
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("malformed extension entries fail closed and preserve the state file", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "sushiai-extension-manager-"));
  try {
    const first = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    await first.ready;
    const stateFile = path.join(dir, "extensions", "extensions.json");
    const malformed = JSON.stringify({
      schemaVersion: 2,
      extensions: { "user.specimen": { enabled: "yes", overrides: {} } },
    });
    await writeFile(stateFile, malformed, "utf8");
    const recovered = new ExtensionManager({
      dataDir: dir,
      installed: [installed],
    });
    const snapshot = await recovered.list();
    assert.equal(snapshot.extensions[0].status, "disabled");
    assert.match(snapshot.diagnostic, /settings were damaged/);
    assert.equal(await readFile(stateFile, "utf8"), malformed);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("an unwritable data directory fails closed instead of crashing the app", async () => {
  const base = await mkdtemp(path.join(tmpdir(), "sushiai-extensions-fail-"));
  const dataDir = path.join(base, "userData");
  await mkdir(dataDir);
  // Readable but not writable: the state file reads as missing (ENOENT), then
  // the first atomic write fails, so init() rejects.
  await chmod(dataDir, 0o500);
  const rejections = [];
  const record = (reason) => rejections.push(reason);
  process.on("unhandledRejection", record);
  try {
    const manager = new ExtensionManager({
      dataDir,
      builtins: [builtin],
      installed: [installed],
    });
    const snapshot = await manager.list();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(rejections.length, 0, "init failure is handled, not thrown");
    assert.match(snapshot.diagnostic, /could not be initialized/i);
    assert.equal(
      snapshot.extensions.find((item) => item.manifest.id === installed.id)
        .status,
      "disabled",
      "external extensions stay off when settings cannot be written",
    );
    assert.equal(
      snapshot.extensions.find((item) => item.manifest.id === builtin.id)
        .status,
      "active",
      "the bundled provider still works",
    );
  } finally {
    process.off("unhandledRejection", record);
    await chmod(dataDir, 0o700).catch(() => {});
    await rm(base, { recursive: true, force: true });
  }
});

const localManifest = (id, version = "1.0.0") => ({
  id,
  name: "Specimens",
  version,
  apiVersion: 1,
  contributions: {
    surfaces: [
      {
        id: "specimen.main",
        title: "Specimens",
        allowedHosts: ["workspace.pane"],
        defaultHost: "workspace.pane",
        instancePolicy: "multiple",
        stateVersion: 1,
        view: {
          kind: "declarative",
          schemaVersion: 1,
          document: { kind: "collection", title: "Tasks", items: [] },
        },
      },
    ],
    navigation: [],
    actions: [],
    commands: [],
  },
});

async function localFolder(root, name, contents) {
  const dir = path.join(root, name);
  await mkdir(dir, { recursive: true });
  await writeFile(
    path.join(dir, "manifest.json"),
    typeof contents === "string" ? contents : JSON.stringify(contents),
  );
  return dir;
}

async function managerFixture(t) {
  const base = await mkdtemp(path.join(tmpdir(), "sushiai-local-manager-"));
  t.after(() => rm(base, { recursive: true, force: true }));
  return {
    dataDir: path.join(base, "data"),
    localDir: path.join(base, "extensions"),
  };
}

const build = (dirs) =>
  new ExtensionManager({
    dataDir: dirs.dataDir,
    builtins: [builtin],
    localDir: dirs.localDir,
  });

test("a local folder arrives disabled and its choice survives a restart", async (t) => {
  const dirs = await managerFixture(t);
  await localFolder(dirs.localDir, "tasks", localManifest("user.specimen"));

  const first = build(dirs);
  const initial = await first.list();
  const record = initial.extensions.find(
    (item) => item.manifest.id === "user.specimen",
  );
  assert.equal(record.status, "disabled", "nothing runs without a choice");
  assert.equal(record.manifest.source.kind, "local");
  await first.setEnabled("user.specimen", true);

  const second = build(dirs);
  assert.equal(
    (await second.list()).extensions.find(
      (item) => item.manifest.id === "user.specimen",
    ).status,
    "active",
  );
});

test("editing the version is a normal edit, not a lock violation", async (t) => {
  const dirs = await managerFixture(t);
  await localFolder(dirs.localDir, "tasks", localManifest("user.specimen"));
  const first = build(dirs);
  await first.setEnabled("user.specimen", true);

  await localFolder(dirs.localDir, "tasks", localManifest("user.specimen", "2.0.0"));
  const second = build(dirs);
  const snapshot = await second.list();
  assert.equal(
    snapshot.extensions.find((item) => item.manifest.id === "user.specimen")
      .status,
    "active",
    "bumping your own version must not disable the extension",
  );
  assert.equal(snapshot.diagnostic, undefined);

  const lock = JSON.parse(
    await readFile(
      path.join(dirs.dataDir, "extensions", "extension-lock.json"),
      "utf8",
    ),
  );
  assert.equal(
    lock.packages["user.specimen"],
    undefined,
    "a local folder is never pinned in the lock",
  );
});

test("a broken manifest is reported and recovers on refresh", async (t) => {
  const dirs = await managerFixture(t);
  await localFolder(dirs.localDir, "tasks", localManifest("user.specimen"));
  const manager = build(dirs);
  await manager.setEnabled("user.specimen", true);

  await localFolder(dirs.localDir, "tasks", "{ broken");
  const broken = await manager.refresh();
  assert.equal(
    broken.extensions.some((item) => item.manifest.id === "user.specimen"),
    false,
    "a manifest that does not parse contributes nothing",
  );
  assert.equal(broken.problems.length, 1);
  assert.equal(broken.problems[0].folder, "tasks");
  assert.ok(
    broken.extensions.some((item) => item.manifest.id === builtin.id),
    "the bundled extension is unaffected",
  );

  await localFolder(dirs.localDir, "tasks", localManifest("user.specimen"));
  const fixed = await manager.refresh();
  assert.deepEqual(fixed.problems, []);
  assert.equal(
    fixed.extensions.find((item) => item.manifest.id === "user.specimen").status,
    "active",
    "the earlier choice is remembered, no need to enable it again",
  );
});

test("refresh picks up a folder added while the app runs", async (t) => {
  const dirs = await managerFixture(t);
  const manager = build(dirs);
  assert.equal((await manager.list()).extensions.length, 1);

  await localFolder(dirs.localDir, "late", localManifest("user.late"));
  const snapshot = await manager.refresh();
  assert.equal(snapshot.extensions.length, 2);
  assert.equal(
    snapshot.extensions.find((item) => item.manifest.id === "user.late").status,
    "disabled",
  );
});

test("a folder cannot shadow the bundled provider", async (t) => {
  const dirs = await managerFixture(t);
  await localFolder(dirs.localDir, "fake", localManifest(builtin.id));
  const manager = build(dirs);
  const snapshot = await manager.list();
  assert.equal(snapshot.problems.length, 1);
  assert.match(snapshot.problems[0].error, /already in use/);
  assert.equal(
    snapshot.extensions.find((item) => item.manifest.id === builtin.id).status,
    "active",
    "the real bundled provider keeps working",
  );
});

test("an unreadable extensions folder does not stop the app", async (t) => {
  const dirs = await managerFixture(t);
  await mkdir(path.dirname(dirs.localDir), { recursive: true });
  await writeFile(dirs.localDir, "not a folder");
  const manager = build(dirs);
  const snapshot = await manager.list();
  assert.equal(snapshot.problems.length, 1);
  assert.match(snapshot.problems[0].error, /unreadable/i);
  assert.equal(
    snapshot.extensions.find((item) => item.manifest.id === builtin.id).status,
    "active",
  );
});
