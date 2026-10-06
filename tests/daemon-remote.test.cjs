const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const { remoteConnectors } = require("../electron/daemon/connectors.cjs");
const { createSshConnector } = require("../electron/daemon/ssh.cjs");
const { createCommandConnector } = require("../electron/daemon/command.cjs");
const { createHostInstaller } = require("../electron/daemon/install.cjs");
const { registerDaemonIpc } = require("../electron/ipc/daemon.cjs");
const { Connections } = require("../electron/connections.cjs");

const cleanups = [];
test.afterEach(async () => {
  while (cleanups.length) await cleanups.pop()();
});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check, ms = 5000, what = "condition") => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await sleep(10);
  }
  throw new Error(`timed out waiting for ${what}`);
};

// A fake ssh (and connector command) over tests/fixtures/fake-proxy.cjs.
function fakeProxy(mode) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fp-"));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  const script = path.join(dir, "ssh");
  fs.writeFileSync(
    script,
    `#!${process.execPath}\nprocess.env.FAKE_PROXY_DIR=${JSON.stringify(dir)};\nrequire(${JSON.stringify(path.join(__dirname, "fixtures/fake-proxy.cjs"))});\n`,
    { mode: 0o755 },
  );
  const api = {
    script,
    dir,
    setMode(next) {
      fs.writeFileSync(path.join(dir, "mode.json"), JSON.stringify(next));
    },
    spawns() {
      try {
        return fs
          .readFileSync(path.join(dir, "spawns.log"), "utf8")
          .trim()
          .split("\n")
          .filter(Boolean).length;
      } catch {
        return 0;
      }
    },
  };
  api.setMode(mode);
  return api;
}

const PROFILE = { id: "host-1", host: "user@devbox", name: "Devbox" };
function sshManager(proxy, options = {}) {
  const manager = createDaemonManager({
    connectors: {
      [PROFILE.id]: createSshConnector({
        profile: PROFILE,
        ssh: proxy.script,
        args: () => ["-T", "-o", "BatchMode=yes"],
        helloTimeoutMs: 3000,
      }),
    },
    backoffMinMs: options.backoffMinMs ?? 20,
    backoffMaxMs: options.backoffMaxMs ?? 200,
    random: () => 1,
  });
  cleanups.push(() => manager.close());
  const seen = [];
  manager.on("state", (state) => seen.push({ ...state }));
  return { manager, seen };
}
const stateOf = (manager, host = PROFILE.id) =>
  manager.states().find((state) => state.host === host);

test("ssh connector: the proxy command runs on the host and the host becomes ready", async () => {
  const proxy = fakeProxy({});
  const { manager } = sshManager(proxy);
  manager.start();
  await until(() => stateOf(manager).state === "ready", 5000, "ready");
  const [args] = fs
    .readFileSync(path.join(proxy.dir, "spawns.log"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  assert.deepEqual(args.slice(0, 3), ["-T", "-o", "BatchMode=yes"]);
  assert.equal(args[3], "user@devbox");
  assert.match(args[4], /^sh -c '.*\.sushiai\/bin\/sushiai.* proxy'$/);
  assert.equal(manager.hello(PROFILE.id).host, "devbox");
});

for (const [name, mode, expected] of [
  [
    "need_auth",
    { exit: 255, stderr: "user@devbox: Permission denied (publickey).\n" },
    { state: "need_auth", reason: undefined },
  ],
  [
    "host_key_changed (identification changed)",
    {
      exit: 255,
      stderr:
        "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.\n",
    },
    { state: "failed", reason: "host_key_changed" },
  ],
  [
    "host_key_changed (verification failed)",
    { exit: 255, stderr: "Host key verification failed.\n" },
    { state: "failed", reason: "host_key_changed" },
  ],
  [
    "not_installed (exit 127)",
    { exit: 127, stderr: "sh: 1: /home/dev/.sushiai/bin/sushiai: not found\n" },
    { state: "failed", reason: "not_installed" },
  ],
  [
    "not_installed (No such file)",
    {
      exit: 1,
      stderr: "/home/dev/.sushiai/bin/sushiai: No such file or directory\n",
    },
    { state: "failed", reason: "not_installed" },
  ],
]) {
  test(`ssh connector: ${name} stops without a retry loop`, async () => {
    const proxy = fakeProxy(mode);
    const { manager } = sshManager(proxy);
    manager.start();
    await until(
      () =>
        stateOf(manager).state === expected.state &&
        stateOf(manager).state !== "connecting",
      5000,
      expected.state,
    );
    assert.equal(stateOf(manager).reason, expected.reason);
    assert.ok(stateOf(manager).message);
    await sleep(400); // many backoff periods at 20 ms
    assert.equal(proxy.spawns(), 1);
    assert.equal(stateOf(manager).state, expected.state);
    // An explicit retry is one more spawn, never a loop.
    await manager.retry(PROFILE.id);
    await sleep(200);
    assert.equal(proxy.spawns(), 2);
  });
}

test("ssh connector: proxy exit 2 reconnects at once, not after the backoff", async () => {
  const proxy = fakeProxy({ dieAfterMs: 150, dieCode: 2 });
  // A backoff of 5 s would fail the wait below.
  const { manager, seen } = sshManager(proxy, {
    backoffMinMs: 5000,
    backoffMaxMs: 5000,
  });
  manager.start();
  await until(() => stateOf(manager).state === "ready", 5000, "first ready");
  proxy.setMode({});
  await until(
    () =>
      stateOf(manager).generation === 2 && stateOf(manager).state === "ready",
    2500,
    "reconnect",
  );
  const offline = seen.find((state) => state.state === "offline");
  assert.equal(offline.reason, "daemon_died");
  assert.equal(proxy.spawns(), 2);
});

test("ssh connector: another failure backs off and keeps trying", async () => {
  const proxy = fakeProxy({
    exit: 1,
    stderr: "ssh: connect to host devbox port 22: Network is unreachable\n",
  });
  const { manager } = sshManager(proxy);
  manager.start();
  await until(() => proxy.spawns() >= 3, 5000, "retries");
  assert.equal(
    stateOf(manager).state === "failed" ||
      stateOf(manager).state === "connecting",
    true,
  );
  assert.match(
    manager.states()[0].message || "Network is unreachable",
    /Network is unreachable/,
  );
  proxy.setMode({});
  await until(() => stateOf(manager).state === "ready", 5000, "ready again");
});

test("an immediate reconnect is granted once: a daemon that dies at once cannot spin", async () => {
  const proxy = fakeProxy({ exit: 2, stderr: "" });
  const { manager } = sshManager(proxy, {
    backoffMinMs: 300,
    backoffMaxMs: 300,
  });
  manager.start();
  await sleep(500);
  // 1 first try + 1 immediate retry + at most 1 after one backoff.
  assert.ok(proxy.spawns() <= 3, `spawns ${proxy.spawns()}`);
});

test("command connector: a healthy command is pinged and stays ready", async () => {
  const proxy = fakeProxy({});
  const manager = createDaemonManager({
    connectors: {
      c1: createCommandConnector({
        profile: {
          connector: { kind: "command", argv: [proxy.script, "--stdio"] },
        },
        pingIntervalMs: 30,
        pingTimeoutMs: 200,
      }),
    },
    backoffMinMs: 20,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "c1").state === "ready");
  await sleep(300);
  assert.equal(stateOf(manager, "c1").state, "ready");
  assert.equal(stateOf(manager, "c1").generation, 1);
  assert.deepEqual(
    JSON.parse(
      fs.readFileSync(path.join(proxy.dir, "spawns.log"), "utf8").trim(),
    ),
    ["--stdio"],
  );
});

test("command connector: an unanswered $/ping drops the connection after the timeout and reconnects", async () => {
  const proxy = fakeProxy({ noPing: true });
  const manager = createDaemonManager({
    connectors: {
      c1: createCommandConnector({
        profile: { connector: { kind: "command", argv: [proxy.script] } },
        pingIntervalMs: 30,
        pingTimeoutMs: 80,
      }),
    },
    backoffMinMs: 20,
    backoffMaxMs: 40,
    random: () => 1,
  });
  cleanups.push(() => manager.close());
  const seen = [];
  manager.on("state", (state) => seen.push({ ...state }));
  manager.start();
  await until(() => stateOf(manager, "c1").state === "ready");
  await until(
    () => seen.some((state) => state.reason === "ping_timeout"),
    3000,
    "ping timeout",
  );
  assert.equal(seen.find((s) => s.reason === "ping_timeout").state, "offline");
  proxy.setMode({});
  await until(
    () =>
      stateOf(manager, "c1").state === "ready" &&
      stateOf(manager, "c1").generation >= 2,
    3000,
    "ready again",
  );
});

test("command connector: a missing command is a failed state naming it", async () => {
  const manager = createDaemonManager({
    connectors: {
      c1: createCommandConnector({
        profile: {
          connector: {
            kind: "command",
            argv: ["/nonexistent/sushiai-connector"],
          },
        },
      }),
    },
    backoffMinMs: 5000,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "c1").state === "failed");
  assert.match(stateOf(manager, "c1").message, /nonexistent/);
});

test("hosts are added, replaced and removed live from the profile list", async () => {
  const one = fakeProxy({});
  const two = fakeProxy({});
  const local = {
    kind: "local",
    connect: () => new Promise(() => {}),
  };
  const build = (profiles, proxy) => ({
    local,
    ...remoteConnectors(profiles, { ssh: proxy.script, args: () => [] }),
  });
  const manager = createDaemonManager({
    connectors: build([], one),
    backoffMinMs: 20,
  });
  cleanups.push(() => manager.close());
  const seen = [];
  manager.on("state", (state) => seen.push({ ...state }));
  manager.start();
  assert.deepEqual(
    manager.states().map((s) => s.host),
    ["local"],
  );

  manager.setConnectors(build([{ id: "h1", host: "a" }], one));
  await until(() => stateOf(manager, "h1")?.state === "ready");
  manager.setConnectors(
    build(
      [
        { id: "h1", host: "a" },
        {
          id: "h2",
          host: "b",
          connector: { kind: "command", argv: [two.script] },
        },
      ],
      one,
    ),
  );
  await until(() => stateOf(manager, "h2")?.state === "ready");
  // An unchanged profile keeps its connection.
  assert.equal(stateOf(manager, "h1").generation, 1);
  assert.equal(one.spawns(), 1);

  // A changed profile replaces the connector and reconnects.
  manager.setConnectors(
    build(
      [
        { id: "h1", host: "other" },
        {
          id: "h2",
          host: "b",
          connector: { kind: "command", argv: [two.script] },
        },
      ],
      one,
    ),
  );
  await until(() => one.spawns() === 2, 3000, "replaced");
  await until(() => stateOf(manager, "h1").state === "ready");

  manager.setConnectors(
    build(
      [
        {
          id: "h2",
          host: "b",
          connector: { kind: "command", argv: [two.script] },
        },
      ],
      one,
    ),
  );
  assert.deepEqual(
    manager
      .states()
      .map((s) => s.host)
      .sort(),
    ["h2", "local"],
  );
  const removed = seen.filter((s) => s.host === "h1").at(-1);
  assert.equal(removed.state, "offline");
  assert.equal(removed.reason, "removed");
  await assert.rejects(manager.request("h1", "session.list"), /unknown host/);
  const spawns = one.spawns();
  await sleep(150);
  assert.equal(one.spawns(), spawns);
});

test("saving an unchanged profile retries a host that stopped without a retry", async () => {
  const proxy = fakeProxy({
    exit: 255,
    stderr: "Permission denied (publickey).\n",
  });
  const profile = { id: "h1", host: "a" };
  const build = () => ({
    ...remoteConnectors([profile], { ssh: proxy.script, args: () => [] }),
  });
  const manager = createDaemonManager({
    connectors: build(),
    backoffMinMs: 20,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "h1").state === "need_auth");
  proxy.setMode({});
  manager.setConnectors(build());
  await until(() => stateOf(manager, "h1").state === "ready");
});

test("a re-attach that fails with a plain error is tried again with backoff", async () => {
  let attaches = 0;
  const handlers = [];
  const makeClient = () => {
    const listeners = {};
    return {
      hello: { daemon: "1", host: "h", capabilities: [] },
      request: async () => [],
      attach: async (id, onBytes) => {
        attaches++;
        if (attaches === 2) throw new Error("socket hiccup");
        onBytes(Buffer.from("screen"), { snapshot: true });
        return { cols: 1, rows: 1, seq: 0n, detach: async () => {} };
      },
      on(name, cb) {
        listeners[name] = cb;
        handlers.push(listeners);
      },
      close() {},
      emit: (name) => listeners[name]?.(),
    };
  };
  const clients = [];
  const manager = createDaemonManager({
    connectors: {
      h: {
        connect: async () => {
          const client = makeClient();
          clients.push(client);
          return client;
        },
      },
    },
    backoffMinMs: 10,
    backoffMaxMs: 20,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "h").state === "ready");
  const got = [];
  await manager.attach("h", "s1", {}, (data, info) => got.push(info));
  clients[0].emit("disconnect");
  await until(
    () => got.filter((i) => i.reattach).length === 1,
    3000,
    "retried re-attach",
  );
  assert.equal(attaches, 3);
});

test("the host-install handler installs, shuts the old daemon down and waits for ready", async () => {
  const order = [];
  let state = { host: "h1", state: "ready" };
  const manager = {
    states: () => [state],
    request: async (host, method) => {
      order.push(`request ${method}`);
      state = { host, state: "offline" };
    },
    retry: async () => {
      order.push("retry");
      state = {
        host: "h1",
        state:
          order.filter((x) => x === "retry").length >= 2 ? "ready" : "offline",
      };
      return state;
    },
  };
  const connections = {
    hasShell: () => true,
    exec: async (endpoint) => {
      order.push(`exec ${endpoint}`);
      return "";
    },
  };
  const installer = createHostInstaller({
    manager,
    connections,
    manifest: () => ({ manifest: { "Linux x86_64": {} }, binDir: "/m" }),
    install: async ({ exec, manifest, binDir }) => {
      assert.equal(binDir, "/m");
      assert.deepEqual(Object.keys(manifest), ["Linux x86_64"]);
      await exec("echo 1", {});
      order.push("install");
      return { status: "installed", version: "0.1.0" };
    },
    settleMs: 1,
  });
  const result = await installer.install("h1");
  assert.equal(result.status, "installed");
  assert.deepEqual(order, [
    "exec ssh:h1",
    "install",
    "request daemon.shutdown",
    "retry",
    "retry",
  ]);
});

test("host-install without a running daemon sends no shutdown; failures are explicit", async () => {
  const base = (over = {}) => {
    const calls = [];
    const manager = {
      states: () => [{ host: "h1", state: "failed", reason: "not_installed" }],
      request: async (...args) => calls.push(args),
      retry: async () => over.final ?? { host: "h1", state: "ready" },
    };
    return {
      calls,
      installer: createHostInstaller({
        manager,
        connections: {
          hasShell: () => over.shell !== false,
          exec: async () => "",
        },
        manifest: over.manifest || (() => ({ manifest: {}, binDir: "/m" })),
        install: async () => ({ status: "unchanged" }),
        settleMs: 1,
        attempts: 3,
      }),
    };
  };
  const ok = base();
  assert.equal((await ok.installer.install("h1")).status, "unchanged");
  assert.equal(ok.calls.length, 0);
  await assert.rejects(ok.installer.install("local"), /ships with the app/);
  await assert.rejects(
    base({
      manifest: () => {
        throw new Error("No host manifest. Run npm run build:host first.");
      },
    }).installer.install("h1"),
    /npm run build:host first/,
  );
  await assert.rejects(
    base({ shell: false }).installer.install("h1"),
    /command/,
  );
  await assert.rejects(
    base({
      final: { host: "h1", state: "need_auth", message: "needs a key" },
    }).installer.install("h1"),
    /not ready: needs a key/,
  );
});

test("host-install IPC dispatches to the installer; session updates and ask answers are type-checked", async () => {
  const handlers = new Map();
  const installed = [];
  registerDaemonIpc({
    handle: (channel, fn) => handlers.set(channel, fn),
    installHost: async (host) => {
      installed.push(host);
      return { status: "installed" };
    },
    getManager: () => ({ states: () => [], request: async () => ({}) }),
  });
  assert.deepEqual(await handlers.get("host-install")("h1"), {
    status: "installed",
  });
  assert.deepEqual(installed, ["h1"]);
  const update = handlers.get("daemon-session-update");
  await update("h1", { id: "s", project: null, group: "g", title: "t" });
  for (const bad of [{ project: 5 }, { group: {} }, { title: ["x"] }])
    await assert.rejects(update("h1", { id: "s", ...bad }), /Invalid/);
  const ask = handlers.get("daemon-ask-respond");
  await ask("h1", {
    sessionId: "s",
    askId: "a",
    decision: "allow",
    message: "ok",
  });
  await ask("h1", { sessionId: "s", askId: "a", decision: "deny" });
  await assert.rejects(
    ask("h1", { sessionId: "s", askId: "a", decision: "allow", message: 5 }),
    /Invalid message/,
  );
  await assert.rejects(
    ask("h1", {
      sessionId: "s",
      askId: "a",
      decision: "allow",
      message: "x".repeat(5000),
    }),
    /Invalid message/,
  );
});

test("connection profiles keep the connector, need no socket, and tell listeners about changes", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "conn-"));
  const connections = new Connections(dir, { ssh: "/nonexistent/ssh" });
  cleanups.push(async () => {
    await connections.close().catch(() => {});
    fs.rmSync(dir, { recursive: true, force: true });
  });
  await connections.init();
  const seen = [];
  connections.onProfilesChange((profiles) =>
    seen.push(profiles.map((p) => p.id)),
  );
  const plain = await connections.save({ host: "user@devbox" });
  assert.equal(plain.socket, "~/.config/herdr/herdr.sock");
  const withSsh = await connections.save({
    host: "b",
    connector: { kind: "ssh" },
  });
  assert.deepEqual(withSsh.connector, { kind: "ssh" });
  const command = await connections.save({
    name: "Tunnel",
    connector: { kind: "command", argv: ["my-tunnel", "--stdio"] },
  });
  assert.deepEqual(command.connector, {
    kind: "command",
    argv: ["my-tunnel", "--stdio"],
  });
  assert.equal(command.host, "command");
  for (const bad of [
    { kind: "telnet" },
    { kind: "command" },
    { kind: "command", argv: [] },
    { kind: "command", argv: [""] },
    { kind: "command", argv: ["ok", 5] },
  ])
    await assert.rejects(
      connections.save({ host: "c", connector: bad }),
      /connector|command/i,
    );
  assert.equal(seen.length, 3);
  // Reloaded from the database.
  const again = new Connections(dir);
  await again.init();
  assert.deepEqual(
    again.list().find((p) => p.id === command.id).connector,
    command.connector,
  );
  await again.close().catch(() => {});
  // A command host has no ssh shell.
  assert.equal(connections.hasShell(`ssh:${command.id}`), false);
  assert.equal(connections.hasShell(`ssh:${plain.id}`), true);
  assert.throws(
    () => connections.exec(`ssh:${command.id}`, "true"),
    /no shell access/,
  );
  await connections.delete(`ssh:${plain.id}`);
  assert.equal(seen.length, 4);
  assert.ok(!seen.at(-1).includes(plain.id));
});
