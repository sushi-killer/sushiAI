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

  manager.setConnectors(
    build([{ id: "h1", host: "a", autoConnect: true }], one),
  );
  await until(() => stateOf(manager, "h1")?.state === "ready");
  manager.setConnectors(
    build(
      [
        { id: "h1", host: "a", autoConnect: true },
        {
          id: "h2",
          host: "b",
          autoConnect: true,
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
        { id: "h1", host: "other", autoConnect: true },
        {
          id: "h2",
          host: "b",
          autoConnect: true,
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
          autoConnect: true,
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
  const profile = { id: "h1", host: "a", autoConnect: true };
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
    setup: async (endpoint, { sushiai }) => {
      assert.equal(endpoint, "ssh:h1");
      assert.equal(sushiai.binDir, "/m");
      assert.deepEqual(Object.keys(sushiai.manifest), ["Linux x86_64"]);
      order.push("setup");
      return {
        sushiai: "installed",
        sushiaiResult: { status: "installed", version: "0.1.0" },
      };
    },
    markSetup: (host) => order.push(`mark ${host}`),
    settleMs: 1,
  });
  const result = await installer.install("h1");
  assert.equal(result.status, "installed");
  assert.deepEqual(order, [
    "setup",
    "mark h1",
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
        setup: async () => ({
          sushiai: over.setupFailed ? "failed" : "unchanged",
          sushiaiError: "disk full",
          sushiaiResult: { status: "unchanged" },
        }),
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
    base({ setupFailed: true }).installer.install("h1"),
    /disk full/,
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
  assert.equal(plain.socket, undefined);
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

// Slice 2 review fixes -----------------------------------------------------

test("a daemon that dies right after ready cannot spin: the retry budget refills only after a stable connection", async () => {
  const proxy = fakeProxy({ dieAfterMs: 80, dieCode: 2 });
  const { manager } = sshManager(proxy, {
    backoffMinMs: 300,
    backoffMaxMs: 300,
  });
  manager.start();
  await sleep(1500);
  // Immediate once, then 300 ms backoffs. Resetting on every "ready" gave
  // a new spawn every ~100 ms (about 15 here).
  assert.ok(proxy.spawns() <= 5, `spawns ${proxy.spawns()}`);
  assert.ok(proxy.spawns() >= 2);
});

test("a connection that stays up refills the budget", async () => {
  const proxy = fakeProxy({});
  const manager = createDaemonManager({
    connectors: {
      [PROFILE.id]: createSshConnector({
        profile: PROFILE,
        ssh: proxy.script,
        helloTimeoutMs: 3000,
      }),
    },
    backoffMinMs: 5000,
    stableMs: 50,
    random: () => 1,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager).state === "ready");
  await sleep(150); // stable now
  proxy.setMode({ dieAfterMs: 60, dieCode: 2 });
  await manager.retry(PROFILE.id);
  await until(() => proxy.spawns() >= 3, 2500, "immediate reconnect again");
});

test("a daemon speaking another protocol is failed/incompatible and never retried", async () => {
  const proxy = fakeProxy({ protocol: 99 });
  const { manager } = sshManager(proxy);
  manager.start();
  await until(
    () =>
      stateOf(manager).state === "failed" &&
      stateOf(manager).reason === "incompatible",
  );
  assert.match(stateOf(manager).message, /protocol 99/);
  await sleep(400);
  assert.equal(proxy.spawns(), 1);
});

test("a changed host key carries the exact ssh-keygen command for the resolved name and the app known_hosts file", async () => {
  const hostKey = {
    exit: 255,
    stderr: "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\n",
  };
  const build = (proxy, profile) =>
    createDaemonManager({
      connectors: {
        h: createSshConnector({
          profile,
          ssh: proxy.script,
          args: () => ["-T"],
          knownHostsFile: "/data/known hosts",
        }),
      },
      backoffMinMs: 20,
    });
  // A non-default port reported by ssh -G: bracketed, plus the file.
  const proxy = fakeProxy({ ...hostKey, gPort: 2222 });
  const manager = build(proxy, { id: "h", host: "user@devbox" });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "h").reason === "host_key_changed");
  assert.equal(
    stateOf(manager, "h").hint,
    "ssh-keygen -R '[devbox.example.test]:2222' -f '/data/known hosts'",
  );
  assert.equal(proxy.spawns(), 1); // -G is not a spawn of the proxy
  // The default port: the plain resolved name.
  const plain = fakeProxy({ ...hostKey, gPort: 22 });
  const second = build(plain, { id: "h", host: "devbox" });
  cleanups.push(() => second.close());
  second.start();
  await until(() => stateOf(second, "h").reason === "host_key_changed");
  assert.equal(
    stateOf(second, "h").hint,
    "ssh-keygen -R 'devbox.example.test' -f '/data/known hosts'",
  );
  // A port saved in the profile wins.
  const saved = fakeProxy({ ...hostKey, gPort: 22 });
  const third = build(saved, { id: "h", host: "devbox", port: 2200 });
  cleanups.push(() => third.close());
  third.start();
  await until(() => stateOf(third, "h").reason === "host_key_changed");
  assert.match(
    stateOf(third, "h").hint,
    /^ssh-keygen -R '\[devbox\.example\.test\]:2200' -f /,
  );
});

test("need_auth asks for a key or the agent and 'No such file' counts only for the sushiai path", async () => {
  const { classifyExit } = require("../electron/daemon/ssh.cjs");
  const auth = classifyExit({
    code: 255,
    stderr: "Permission denied (publickey).",
  });
  assert.equal(auth.state, "need_auth");
  assert.match(auth.message, /key or the ssh agent/);
  assert.doesNotMatch(auth.message, /password/i);
  assert.equal(
    classifyExit({
      code: 1,
      stderr: "/home/dev/.sushiai/bin/sushiai: No such file or directory",
    }).reason,
    "not_installed",
  );
  assert.equal(
    classifyExit({
      code: 1,
      stderr:
        "ssh: Could not open /home/dev/.ssh/id: No such file or directory",
    }).reason,
    undefined,
  );
});

test("hosts of profiles without autoConnect wait for a connect; disconnect suspends until the next one", async () => {
  const proxy = fakeProxy({});
  const profile = { id: "h1", host: "a", autoConnect: false };
  const build = (p) =>
    remoteConnectors([p], { ssh: proxy.script, args: () => [] });
  const manager = createDaemonManager({
    connectors: build(profile),
    backoffMinMs: 20,
  });
  cleanups.push(() => manager.close());
  const seen = [];
  manager.on("state", (state) => seen.push({ ...state }));
  manager.start();
  await sleep(150);
  assert.equal(proxy.spawns(), 0);
  assert.equal(stateOf(manager, "h1").state, "offline");
  // Saving the profile again does not connect it either.
  manager.setConnectors(build(profile));
  await sleep(100);
  assert.equal(proxy.spawns(), 0);

  const state = await manager.retry("h1");
  assert.equal(state.state, "ready");
  manager.disconnect("h1");
  assert.equal(stateOf(manager, "h1").state, "offline");
  assert.equal(stateOf(manager, "h1").reason, "disconnected");
  assert.equal(seen.at(-1).reason, "disconnected");
  manager.setConnectors(build({ ...profile, autoConnect: true }));
  await sleep(150);
  // Suspended: not even an autoConnect profile reconnects until a connect.
  assert.equal(proxy.spawns(), 1);
  await manager.retry("h1");
  assert.equal(proxy.spawns(), 2);
  assert.equal(stateOf(manager, "h1").state, "ready");
});

test("an autoConnect profile connects when the manager starts", async () => {
  const proxy = fakeProxy({});
  const manager = createDaemonManager({
    connectors: remoteConnectors([{ id: "h1", host: "a", autoConnect: true }], {
      ssh: proxy.script,
      args: () => [],
    }),
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => stateOf(manager, "h1").state === "ready");
});

test("the old-daemon stop script signals only a daemon that holds the lock", async () => {
  const { spawn, execFileSync } = require("node:child_process");
  const { STOP_DAEMON_COMMAND } = require("../electron/daemon/install.cjs");
  const home = fs.mkdtempSync(path.join("/tmp", "stop-"));
  cleanups.push(() => fs.rmSync(home, { recursive: true, force: true }));
  fs.mkdirSync(path.join(home, ".sushiai"), { mode: 0o700 });
  const run = () =>
    execFileSync("/bin/sh", ["-c", STOP_DAEMON_COMMAND], {
      env: { HOME: home, PATH: process.env.PATH },
      encoding: "utf8",
    }).trim();
  assert.equal(run(), "nolock");
  // A stale pid in an unheld lock is never signalled.
  const bystander = spawn("sleep", ["30"]);
  cleanups.push(() => bystander.kill());
  const lock = path.join(home, ".sushiai", "daemon.lock");
  fs.writeFileSync(lock, String(bystander.pid));
  assert.equal(run(), "free");
  await sleep(100);
  assert.equal(bystander.exitCode, null);
  assert.equal(bystander.signalCode, null);
  // A held lock names a live daemon: it gets SIGTERM.
  const holder = spawn(
    "python3",
    [
      "-c",
      `import fcntl,os,sys,time
f=open(${JSON.stringify(lock)},"r+")
fcntl.flock(f,fcntl.LOCK_EX)
f.seek(0); f.truncate(); f.write(str(os.getpid())); f.flush()
print("held",flush=True)
time.sleep(30)`,
    ],
    { stdio: ["ignore", "pipe", "inherit"] },
  );
  cleanups.push(() => holder.kill());
  await new Promise((resolve) => holder.stdout.once("data", resolve));
  assert.equal(run(), "stopped");
  await until(() => holder.signalCode === "SIGTERM", 3000, "holder stopped");
});

test("a port forward is its own process and never needs the Herdr tunnel", async () => {
  const net = require("node:net");
  const { EventEmitter } = require("node:events");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fwd-"));
  const connections = new Connections(dir, { ssh: "/nonexistent/ssh" });
  const servers = [];
  cleanups.push(async () => {
    for (const server of servers) server.close();
    await connections.close().catch(() => {});
    fs.rmSync(dir, { recursive: true, force: true });
  });
  await connections.init();
  const profile = await connections.save({ host: "user@devbox" });
  const endpoint = `ssh:${profile.id}`;
  connections.socket = async () => {
    throw new Error("the Herdr socket must not be needed");
  };
  const specs = [];
  const killed = [];
  connections.forwardProcess = async (_profile, spec) => {
    specs.push(spec);
    const [, local] = spec.split(":");
    const server = net.createServer((c) => c.destroy());
    servers.push(server);
    await new Promise((resolve) =>
      server.listen(Number(local), "127.0.0.1", resolve),
    );
    const proc = new EventEmitter();
    proc.stderr = new EventEmitter();
    proc.kill = async () => {
      killed.push(spec);
      server.close();
      proc.emit("exit");
    };
    return proc;
  };
  const local = await connections.forward(endpoint, 5173);
  assert.equal(specs.length, 1);
  assert.match(
    specs[0],
    new RegExp(`^127\\.0\\.0\\.1:${local}:127\\.0\\.0\\.1:5173$`),
  );
  assert.equal(await connections.forward(endpoint, 5173), local);
  assert.equal(specs.length, 1);
  await connections.disconnect(endpoint);
  assert.equal(killed.length, 1);
  assert.equal(connections.forwards.has(profile.id), false);
});

test("tool setup runs once per host, when it first becomes ready, and command hosts are skipped", async () => {
  const { EventEmitter } = require("node:events");
  const { watchHostTools } = require("../electron/daemon/host-tools.cjs");
  const bus = new EventEmitter();
  const manager = {
    on: (name, cb) => (bus.on(name, cb), () => bus.off(name, cb)),
  };
  const calls = [];
  const logs = [];
  let stored = {};
  const tools = watchHostTools({
    manager,
    connections: {
      hasShell: (endpoint) => {
        if (endpoint === "ssh:gone") throw new Error("unknown");
        return endpoint !== "ssh:cmd";
      },
    },
    store: { read: () => stored, write: (value) => (stored = value) },
    setup: async (endpoint, options) => {
      calls.push({ endpoint, options });
    },
    log: (message) => logs.push(message),
  });
  cleanups.push(() => tools.off());
  const ready = (host) => bus.emit("state", { host, state: "ready" });
  ready("local");
  ready("devbox");
  await sleep(20);
  ready("devbox"); // a reconnect
  ready("cmd");
  ready("gone");
  await sleep(20);
  assert.deepEqual(calls, [
    { endpoint: "ssh:devbox", options: { sushiai: null } },
  ]);
  assert.deepEqual(stored, { devbox: true });
  assert.ok(logs.some((line) => /cmd.*command/.test(line)));
  // The installer records it too, so a later ready does nothing.
  tools.mark("other");
  ready("other");
  await sleep(20);
  assert.equal(calls.length, 1);
});
