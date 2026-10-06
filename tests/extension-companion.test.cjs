const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const {
  createCompanions,
  resolveCommand,
} = require("../electron/extensions/companion-process.cjs");
const { listSshHosts } = require("../electron/extensions/hosts.cjs");
const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");
const { registerExtensionIpc } = require("../electron/ipc/extensions.cjs");

const FIXTURES = path.join(__dirname, "fixtures/extensions/companion-probe");
const MANIFEST = JSON.parse(
  fs.readFileSync(path.join(FIXTURES, "manifest.json"), "utf8"),
);
const ID = MANIFEST.id;
const SURFACE = "probe.companion-settings";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(check, what, ms = 8000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(15);
  }
}
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** A data dir, an extensions folder holding the fixture manifest, and a
 * SUSHIAI_HOME whose bin/ holds the synthetic companion. */
function layout(t, { manifest = MANIFEST, install = true } = {}) {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-companion-"));
  const dirs = {
    base,
    dataDir: path.join(base, "data"),
    localDir: path.join(base, "extensions"),
    home: path.join(base, "home"),
  };
  fs.mkdirSync(path.join(dirs.localDir, "probe"), { recursive: true });
  fs.mkdirSync(path.join(dirs.home, "bin"), { recursive: true });
  const write = (next) =>
    fs.writeFileSync(
      path.join(dirs.localDir, "probe", "manifest.json"),
      JSON.stringify(next),
    );
  write(manifest);
  if (install) {
    const command = path.join(dirs.home, "bin", "probe-companion");
    fs.writeFileSync(
      command,
      `#!/bin/sh\nexec "${process.execPath}" "${path.join(FIXTURES, "companion.cjs")}" "$@"\n`,
      { mode: 0o755 },
    );
  }
  const probe = (name) => path.join(dirs.home, "probe", name);
  const pids = () =>
    fs.existsSync(probe("starts"))
      ? fs
          .readFileSync(probe("starts"), "utf8")
          .split("\n")
          .filter(Boolean)
          .map(Number)
      : [];
  const control = (name) => {
    fs.mkdirSync(path.dirname(probe(name)), { recursive: true });
    fs.writeFileSync(probe(name), "");
  };
  const calls = () =>
    fs.existsSync(probe("calls"))
      ? fs
          .readFileSync(probe("calls"), "utf8")
          .split("\n")
          .filter(Boolean)
          .map((line) => JSON.parse(line))
      : [];
  t.after(() => {
    for (const pid of pids()) if (alive(pid)) process.kill(pid, "SIGKILL");
    fs.rmSync(base, { recursive: true, force: true });
  });
  return { ...dirs, write, probe, pids, control, calls };
}

const PROFILES = [
  { id: "a-1", name: "Alpha", host: "alpha.example.test", port: 2222 },
  { id: "b-2", name: "Beta", host: "beta.example.test" },
  {
    id: "c-3",
    name: "Gamma",
    host: "gamma",
    connector: { kind: "command", argv: ["gamma-shell"] },
  },
];

/** A manager with a started companion supervisor over the layout. */
async function build(t, dirs, options = {}) {
  const companions = createCompanions({
    home: dirs.home,
    getHosts: () => PROFILES,
    env: {
      PATH: "",
      HOME: dirs.base,
      USER: "tester",
      LANG: "C",
      TMPDIR: os.tmpdir(),
      SUSHIAI_HOME: dirs.home,
      SECRET_TOKEN: "must-not-reach-the-child",
    },
    backoffMs: [20, 20, 20],
    killGraceMs: 300,
    helloTimeoutMs: 4000,
    ...options,
  });
  const manager = new ExtensionManager({
    dataDir: dirs.dataDir,
    localDir: dirs.localDir,
    companions,
  });
  t.after(() => companions.stopAll());
  await manager.ready;
  await companions.start();
  return { manager, companions };
}

const companionOf = async (manager) =>
  (await manager.list()).extensions.find((item) => item.manifest.id === ID)
    .companion;

test("the companion is not started before approval, and starts after it", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  const before = await companionOf(manager);
  assert.equal(before.state, "needs-approval");
  assert.equal(
    before.resolvedPath,
    path.join(dirs.home, "bin", "probe-companion"),
  );
  assert.deepEqual(before.args, ["serve"]);
  assert.deepEqual(before.permissions, ["hosts.read"]);
  await sleep(150);
  assert.deepEqual(dirs.pids(), [], "nothing ran without consent");
  await assert.rejects(() => manager.companionRead(ID, SURFACE), /not running/);

  await manager.approve(ID);
  assert.equal((await companionOf(manager)).state, "running");
  assert.equal(dirs.pids().length, 1);
  const state = JSON.parse(
    fs.readFileSync(path.join(dirs.home, "probe", "started-with"), "utf8"),
  );
  assert.deepEqual(state.args, ["serve"]);
});

test("a disabled extension never starts, even with an approval on record", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.approve(ID);
  assert.equal((await companionOf(manager)).state, "off");
  assert.deepEqual(dirs.pids(), []);
});

test("an approval survives a restart of the manager", async (t) => {
  const dirs = layout(t);
  const first = await build(t, dirs);
  await first.manager.setEnabled(ID, true);
  await first.manager.approve(ID);
  await first.companions.stopAll();
  const second = await build(t, dirs);
  assert.equal((await companionOf(second.manager)).state, "running");
});

test("the child gets the environment allowlist and nothing else", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const { env } = JSON.parse(
    fs.readFileSync(path.join(dirs.home, "probe", "started-with"), "utf8"),
  );
  assert.ok(!env.includes("SECRET_TOKEN"));
  for (const key of env)
    assert.ok(
      [
        "HOME",
        "LANG",
        "SUSHIAI_HOME",
        "TMPDIR",
        "USER",
        "PWD",
        "SHLVL",
        "_",
        "__CF_USER_TEXT_ENCODING",
      ].includes(key) || key === "PATH",
      `unexpected variable ${key}`,
    );
  assert.ok(env.includes("SUSHIAI_HOME"));
});

test("view.read round-trips and checks the reply against the declared fields", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const result = await manager.companionRead(ID, SURFACE);
  assert.deepEqual(result.values, {
    hub: { text: "Linked", tone: "ok" },
    pairing: "probe-pairing-code",
    note: null,
  });
  await assert.rejects(
    () => manager.companionRead(ID, "no.such.surface"),
    /No active companion view/,
  );
});

test('send: ["hosts"] delivers the ssh hosts and no command host; the renderer names no method', async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const result = await manager.companionAction(ID, SURFACE, "import-hosts");
  assert.equal(result.message, "imported");
  assert.deepEqual(result.values, { note: "2 hosts" });
  const [call] = dirs.calls();
  assert.equal(call.method, "hosts.import");
  assert.deepEqual(call.params, {
    surfaceId: SURFACE,
    hosts: [
      { id: "a-1", name: "Alpha", host: "alpha.example.test", port: 2222 },
      { id: "b-2", name: "Beta", host: "beta.example.test" },
    ],
  });
  // An action without send gets no hosts.
  await manager.companionAction(ID, SURFACE, "add-device");
  assert.deepEqual(dirs.calls()[1].params, { surfaceId: SURFACE });
  await assert.rejects(
    () => manager.companionAction(ID, SURFACE, "hosts.import"),
    /Unknown companion action/,
  );
});

test("listSshHosts keeps ssh profiles only", () => {
  assert.deepEqual(
    listSshHosts(() => PROFILES).map((host) => host.id),
    ["a-1", "b-2"],
  );
  assert.deepEqual(
    listSshHosts(() => undefined),
    [],
  );
});

test("view.changed from the companion reaches the listener", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const seen = [];
  const stop = manager.onCompanionChanged((change) => seen.push(change));
  await manager.companionAction(ID, SURFACE, "add-device");
  await waitFor(() => seen.length, "view.changed");
  assert.deepEqual(seen[0], { extensionId: ID, surfaceId: SURFACE });
  stop();
});

test("a changed args value or permission forces a new approval", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const [pid] = dirs.pids();
  assert.ok(alive(pid));

  dirs.write({
    ...MANIFEST,
    companion: { ...MANIFEST.companion, args: ["serve", "--more"] },
  });
  await manager.refresh();
  const after = await companionOf(manager);
  assert.equal(after.state, "needs-approval");
  assert.deepEqual(after.args, ["serve", "--more"]);
  await waitFor(() => !alive(pid), "the old process to stop");
  assert.equal(dirs.pids().length, 1, "no second start without consent");

  await manager.approve(ID);
  assert.equal((await companionOf(manager)).state, "running");

  dirs.write({
    ...MANIFEST,
    companion: {
      ...MANIFEST.companion,
      args: ["serve", "--more"],
      permissions: [],
    },
    contributions: {
      ...MANIFEST.contributions,
      surfaces: [
        {
          ...MANIFEST.contributions.surfaces[0],
          view: { ...MANIFEST.contributions.surfaces[0].view, actions: [] },
        },
      ],
    },
  });
  await manager.refresh();
  assert.equal((await companionOf(manager)).state, "needs-approval");
});

test("a command that resolves inside the extension folder is refused", async (t) => {
  const dirs = layout(t, { install: false });
  // The real command is a symlink into the extension folder.
  const inside = path.join(dirs.localDir, "probe", "run.sh");
  fs.writeFileSync(inside, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  fs.symlinkSync(inside, path.join(dirs.home, "bin", "probe-companion"));
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  const status = await companionOf(manager);
  assert.equal(status.state, "failed");
  assert.match(status.stderrTail, /inside the extension folder/);
  assert.equal(status.resolvedPath, undefined);
  await assert.rejects(() => manager.approve(ID), /cannot be resolved/);
  assert.deepEqual(dirs.pids(), []);
});

test("resolution prefers $SUSHIAI_HOME/bin over PATH", (t) => {
  const dirs = layout(t);
  const other = path.join(dirs.base, "other");
  fs.mkdirSync(other);
  fs.writeFileSync(path.join(other, "probe-companion"), "#!/bin/sh\n", {
    mode: 0o755,
  });
  const found = resolveCommand("probe-companion", {
    home: dirs.home,
    pathEnv: other,
  });
  assert.equal(found.path, path.join(dirs.home, "bin", "probe-companion"));
  fs.rmSync(path.join(dirs.home, "bin", "probe-companion"));
  assert.equal(
    resolveCommand("probe-companion", { home: dirs.home, pathEnv: other }).path,
    path.join(other, "probe-companion"),
  );
  assert.match(
    resolveCommand("probe-companion", { home: dirs.home, pathEnv: "" }).error,
    /not found/,
  );
});

test("kill -9 restarts the companion", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const [first] = dirs.pids();
  process.kill(first, "SIGKILL");
  await waitFor(async () => dirs.pids().length === 2, "a restart");
  await waitFor(
    async () => (await companionOf(manager)).state === "running",
    "running again",
  );
  assert.ok(alive(dirs.pids()[1]));
  assert.equal(
    (await manager.companionRead(ID, SURFACE)).values.pairing,
    "probe-pairing-code",
  );
});

test("three exits in ten minutes end in failed, with the stderr tail", async (t) => {
  const dirs = layout(t);
  dirs.control("exit-now");
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const status = await waitFor(async () => {
    const current = await companionOf(manager);
    return current.state === "failed" && current;
  }, "failed");
  assert.match(status.stderrTail, /boom: probe companion refused to start/);
  assert.equal(dirs.pids().length, 3);
  await sleep(120);
  assert.equal(dirs.pids().length, 3, "it stops trying once failed");
  // Switching it off and on again is a fresh start.
  fs.rmSync(dirs.probe("exit-now"));
  await manager.setEnabled(ID, false);
  await manager.setEnabled(ID, true);
  assert.equal((await companionOf(manager)).state, "running");
});

test("disable removes the process within 3 seconds, even one that ignores SIGTERM", async (t) => {
  const dirs = layout(t);
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const [pid] = dirs.pids();
  const began = Date.now();
  await manager.setEnabled(ID, false);
  assert.ok(Date.now() - began < 3000);
  assert.ok(!alive(pid));
  assert.equal((await companionOf(manager)).state, "off");

  dirs.control("ignore-term");
  await manager.setEnabled(ID, true);
  const stubborn = dirs.pids().at(-1);
  assert.notEqual(stubborn, pid);
  const started = Date.now();
  await manager.setEnabled(ID, false);
  assert.ok(Date.now() - started < 3000);
  assert.ok(!alive(stubborn), "SIGKILL ends what SIGTERM did not");
});

test("quitting stops the process", async (t) => {
  const dirs = layout(t);
  const { manager, companions } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  const [pid] = dirs.pids();
  await companions.stopAll();
  assert.ok(!alive(pid));
});

test("companion values are never logged", async (t) => {
  const dirs = layout(t);
  const lines = [];
  const originals = ["log", "info", "warn", "error"].map((name) => [
    name,
    console[name],
  ]);
  for (const [name] of originals)
    console[name] = (...args) => lines.push(args.join(" "));
  t.after(() => originals.forEach(([name, fn]) => (console[name] = fn)));
  const { manager } = await build(t, dirs);
  await manager.setEnabled(ID, true);
  await manager.approve(ID);
  await manager.companionRead(ID, SURFACE);
  assert.ok(!lines.join("\n").includes("probe-pairing-code"));
});

test("the IPC handlers pass ids only and validate them", async () => {
  const handlers = new Map();
  const seen = [];
  const manager = {
    companionRead: async (...a) => (seen.push(["read", ...a]), { values: {} }),
    companionAction: async (...a) => (seen.push(["action", ...a]), {}),
    approve: async (...a) => void seen.push(["approve", ...a]),
    onCompanionChanged: (listener) => (
      seen.push(["subscribed"]),
      listener({ extensionId: "x", surfaceId: "y" })
    ),
  };
  registerExtensionIpc({
    handle: (channel, fn) => handlers.set(channel, fn),
    getExtensions: () => manager,
    getSurfaceState: () => ({}),
    announceCompanion: (change) => seen.push(["announce", change]),
  });
  await handlers.get("extensions-companion-read")("e", "s");
  await handlers.get("extensions-companion-action")("e", "s", "a");
  assert.equal(await handlers.get("extensions-approve")("e"), undefined);
  assert.deepEqual(seen, [
    ["subscribed"],
    ["announce", { extensionId: "x", surfaceId: "y" }],
    ["read", "e", "s"],
    ["action", "e", "s", "a"],
    ["approve", "e"],
  ]);
  await assert.rejects(
    async () => handlers.get("extensions-companion-action")(1, "s", "a"),
    /Invalid extension id/,
  );
  await assert.rejects(
    async () => handlers.get("extensions-approve")(""),
    /Invalid extension id/,
  );
});

const base = (patch = {}) => ({
  ...structuredClone(MANIFEST),
  source: { kind: "local", path: "/tmp/x" },
  ...patch,
});
const view = (patch) => {
  const copy = base();
  Object.assign(copy.contributions.surfaces[0].view, patch);
  return copy;
};

test("the validator rules for companion blocks and views", () => {
  const ok = (input) => validateExtensionManifest(input);
  assert.deepEqual(
    ok(
      view({
        actions: [{ id: "go", label: "Go", method: "x.go" }],
      }),
    ).companion,
    MANIFEST.companion,
  );
  const plain = (companion) =>
    ok({
      ...base({ companion }),
      contributions: {
        ...MANIFEST.contributions,
        surfaces: [
          {
            ...MANIFEST.contributions.surfaces[0],
            view: { kind: "companion", fields: [], actions: [] },
          },
        ],
      },
    }).companion;
  assert.deepEqual(plain({ command: "tool" }), {
    command: "tool",
    args: [],
    permissions: [],
  });
  for (const [companion, message] of [
    [{ command: "/bin/tool" }, /companion\.command/],
    [{ command: "../tool" }, /companion\.command/],
    [{ command: "tool", args: Array(9).fill("a") }, /at most 8/],
    [{ command: "tool", args: ["a".repeat(201)] }, /at most 200/],
    [{ command: "tool", args: [1] }, /strings/],
    [{ command: "tool", permissions: ["fs.write"] }, /not a permission/],
    [{ command: "tool", permissions: ["hosts.read", "hosts.read"] }, /twice/],
  ])
    assert.throws(() => plain(companion), message);
  assert.throws(
    () => ok({ ...base(), source: { kind: "builtin" } }),
    /Built-in extensions may not declare a companion/,
  );
});

test("the validator rejects a companion view off settings.page", () => {
  const manifest = base();
  manifest.contributions.surfaces[0].allowedHosts = [
    "settings.page",
    "app.page",
  ];
  assert.throws(
    () => validateExtensionManifest(manifest),
    /only allowed on settings\.page/,
  );
  const pane = base();
  pane.contributions.surfaces[0].allowedHosts = ["workspace.pane"];
  pane.contributions.surfaces[0].defaultHost = "workspace.pane";
  assert.throws(
    () => validateExtensionManifest(pane),
    /only allowed on settings\.page/,
  );
});

test("the validator rejects a companion view without a companion block", () => {
  const manifest = base();
  delete manifest.companion;
  assert.throws(
    () => validateExtensionManifest(manifest),
    /needs a top-level companion block/,
  );
});

test('the validator rejects send: ["hosts"] without hosts.read', () => {
  const manifest = base();
  manifest.companion.permissions = [];
  assert.throws(
    () => validateExtensionManifest(manifest),
    /needs the hosts\.read permission/,
  );
});

test("the validator caps fields at 8 and actions at 4, and checks their shape", () => {
  const fields = (count) =>
    Array.from({ length: count }, (_, index) => ({
      id: `f${index}`,
      label: `F${index}`,
      type: "text",
    }));
  const actions = (count) =>
    Array.from({ length: count }, (_, index) => ({
      id: `a${index}`,
      label: `A${index}`,
      method: "do.it",
    }));
  assert.ok(
    validateExtensionManifest(view({ fields: fields(8), actions: actions(4) })),
  );
  assert.throws(
    () => validateExtensionManifest(view({ fields: fields(9), actions: [] })),
    /at most 8/,
  );
  assert.throws(
    () => validateExtensionManifest(view({ fields: [], actions: actions(5) })),
    /at most 4/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        view({
          fields: [],
          actions: [{ id: "a", label: "A", method: "Do.It" }],
        }),
      ),
    /method must match/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        view({
          fields: [],
          actions: [
            { id: "a", label: "Same", method: "x" },
            { id: "b", label: "Same", method: "y" },
          ],
        }),
      ),
    /Duplicate extension action label/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        view({ fields: [{ id: "f", label: "F", type: "image" }], actions: [] }),
      ),
    /not one of|type/i,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        view({
          fields: [],
          actions: [{ id: "a", label: "A", method: "x", send: ["projects"] }],
        }),
      ),
    /send may only list/,
  );
});
