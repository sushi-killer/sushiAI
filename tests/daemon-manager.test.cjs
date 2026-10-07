const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { EventEmitter } = require("node:events");
const { connectDaemon } = require("../electron/daemon/client.cjs");
const { createDecoder, encode } = require("../electron/daemon/frame.cjs");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const {
  createLocalConnector,
  resolveBinary,
  ensureHome,
  HOOKS_STAMP,
} = require("../electron/daemon/local.cjs");

const cleanups = [];
test.afterEach(async () => {
  while (cleanups.length) await cleanups.pop()();
});

function tmp() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sd-"));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

const until = async (check, ms = 3000, what = "condition") => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`timed out waiting for ${what}`);
};

// A fake daemon on a unix socket, built on the frame codec. `shutdown` removes
// the socket like the real daemon does.
async function fakeDaemon(
  socketPath,
  { version = "1.0.0", host = "devbox", build } = {},
) {
  const conns = [];
  const log = [];
  const server = net.createServer((socket) => {
    const decoder = createDecoder();
    conns.push(socket);
    socket.on("error", () => {});
    const reply = (id, result) =>
      socket.write(
        encode({
          kind: "J",
          json: JSON.stringify({ jsonrpc: "2.0", id, result }),
        }),
      );
    socket.on("data", (chunk) => {
      for (const frame of decoder.push(chunk)) {
        const message = JSON.parse(frame.json);
        log.push(message);
        if (message.method === "hello")
          reply(message.id, {
            protocol: 1,
            capabilities: ["sessions", "attach"],
            daemon: version,
            host,
            ...(build ? { build } : {}),
          });
        else if (message.method === "session.list") reply(message.id, []);
        else if (message.method === "session.attach")
          reply(message.id, {
            snapshot: Buffer.from(`screen-${log.length}`).toString("base64"),
            seq: 5,
            cols: 80,
            rows: 24,
          });
        else if (message.method === "daemon.shutdown") {
          reply(message.id, {});
          setTimeout(() => daemon.stop(), 20);
        } else reply(message.id, {});
      }
    });
  });
  const daemon = {
    log,
    version,
    notify(method, params) {
      for (const socket of conns)
        socket.write(
          encode({
            kind: "J",
            json: JSON.stringify({ jsonrpc: "2.0", method, params }),
          }),
        );
    },
    dropConnections() {
      for (const socket of conns.splice(0)) socket.destroy();
    },
    async stop() {
      this.dropConnections();
      if (!server.listening) return;
      await new Promise((resolve) => server.close(resolve));
      fs.rmSync(socketPath, { force: true });
    },
  };
  fs.rmSync(socketPath, { force: true });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  cleanups.push(() => daemon.stop());
  return daemon;
}

// Connector stub: connects to a fake daemon socket that can be replaced between tries.
function stubConnector(target) {
  return {
    kind: "stub",
    connect: () =>
      connectDaemon({ socketPath: target.socketPath, clientName: "desktop" }),
  };
}

function startManager(connector, options = {}) {
  const manager = createDaemonManager({
    connectors: { local: connector },
    backoffMinMs: 10,
    backoffMaxMs: 40,
    ...options,
  });
  cleanups.push(() => manager.close());
  return manager;
}

const stateOf = (manager) => manager.states()[0];

test("hello and session.list make the host ready with version, capabilities and generation", async () => {
  const socketPath = path.join(tmp(), "d.sock");
  const daemon = await fakeDaemon(socketPath, { version: "2.3.4" });
  const manager = startManager(stubConnector({ socketPath }));
  const seen = [];
  manager.on("state", (state) => seen.push(state.state));
  manager.start();
  await until(() => stateOf(manager).state === "ready", 3000, "ready");
  assert.deepEqual(seen, ["connecting", "ready"]);
  assert.deepEqual(stateOf(manager), {
    host: "local",
    state: "ready",
    generation: 1,
    version: "2.3.4",
    capabilities: ["sessions", "attach"],
  });
  assert.deepEqual(manager.hello("local"), {
    host: "devbox",
    version: "2.3.4",
    capabilities: ["sessions", "attach"],
  });
  const hello = daemon.log.find((m) => m.method === "hello");
  assert.equal(hello.params.client, "desktop");
  assert.deepEqual(
    daemon.log.map((m) => m.method),
    ["hello", "session.list"],
  );
  assert.deepEqual(await manager.request("local", "session.list", {}), []);
  await assert.rejects(
    manager.request("other", "session.list"),
    /unknown host/,
  );
});

test("setUpdate flags a ready host with update and a new connection clears it", async () => {
  const socketPath = path.join(tmp(), "d.sock");
  await fakeDaemon(socketPath, { build: "a".repeat(64) });
  const manager = startManager(stubConnector({ socketPath }));
  manager.start();
  await until(() => stateOf(manager).state === "ready", 3000, "ready");
  assert.equal(manager.hello("local").build, "a".repeat(64));
  assert.equal(stateOf(manager).update, undefined);
  const seen = [];
  manager.on("state", (state) => seen.push(state.update));
  manager.setUpdate("local", true);
  manager.setUpdate("local", true);
  assert.deepEqual(seen, [true]);
  assert.equal(stateOf(manager).update, true);
  await manager.retry("local");
  assert.equal(stateOf(manager).update, undefined);
});

test("notifications are forwarded as events with the generation", async () => {
  const socketPath = path.join(tmp(), "d.sock");
  const daemon = await fakeDaemon(socketPath);
  const manager = startManager(stubConnector({ socketPath }));
  const events = [];
  const off = manager.on("event", (event) => events.push(event));
  manager.start();
  await until(() => stateOf(manager).state === "ready");
  daemon.notify("session.created", { id: "s1" });
  daemon.notify("session.snapshot", { id: "s1", snapshot: "", seq: 0 });
  await until(() => events.length >= 1);
  assert.deepEqual(events, [
    {
      host: "local",
      generation: 1,
      method: "session.created",
      params: { id: "s1" },
    },
  ]);
  off();
  daemon.notify("session.removed", { id: "s1" });
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(events.length, 1);
});

test("requests fail while the host is not ready", async () => {
  const manager = startManager({
    kind: "never",
    connect: () => new Promise(() => {}),
  });
  manager.start();
  await assert.rejects(
    manager.request("local", "session.list"),
    /not ready \(connecting\)/,
  );
  await assert.rejects(
    manager.attach("local", "s1", {}, () => {}),
    /not ready/,
  );
});

test("a lost connection goes offline, backs off, reconnects with generation+1 and re-attaches with a snapshot", async () => {
  const socketPath = path.join(tmp(), "d.sock");
  const daemon = await fakeDaemon(socketPath);
  const manager = startManager(stubConnector({ socketPath }));
  const seen = [];
  manager.on("state", (state) =>
    seen.push(`${state.state}:${state.generation}`),
  );
  manager.start();
  await until(() => stateOf(manager).state === "ready");
  const received = [];
  const handle = await manager.attach(
    "local",
    "s1",
    { scrollback: 300 },
    (data, info) => received.push({ text: data.toString(), ...info }),
  );
  assert.equal(handle.cols, 80);
  assert.equal(
    daemon.log.find((m) => m.method === "session.attach").params.scrollback,
    300,
  );
  assert.equal(received.length, 1);
  assert.equal(received[0].snapshot, true);
  assert.equal(received[0].generation, 1);
  assert.equal(received[0].reattach, undefined);

  daemon.dropConnections();
  await until(
    () =>
      stateOf(manager).generation === 2 && stateOf(manager).state === "ready",
  );
  assert.deepEqual(seen, [
    "connecting:0",
    "ready:1",
    "offline:1",
    "connecting:1",
    "ready:2",
  ]);
  await until(() => received.length === 2, 3000, "re-attach snapshot");
  assert.equal(received[1].snapshot, true);
  assert.equal(received[1].reattach, true);
  assert.equal(received[1].generation, 2);
  assert.match(received[1].text, /^screen-/);
  const attaches = daemon.log.filter((m) => m.method === "session.attach");
  assert.equal(attaches.length, 2);
  assert.equal(attaches[1].params.scrollback, 2000);

  // After detach the handle is no longer re-attached.
  await handle.detach();
  daemon.dropConnections();
  await until(
    () =>
      stateOf(manager).generation === 3 && stateOf(manager).state === "ready",
  );
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(
    daemon.log.filter((m) => m.method === "session.attach").length,
    2,
  );
});

test("backoff doubles up to the maximum with jitter and a failed connect is reported", async () => {
  const delays = [];
  const realSetTimeout = global.setTimeout;
  global.setTimeout = (fn, ms, ...rest) => {
    if (ms >= 100) delays.push(ms);
    return realSetTimeout(fn, 1, ...rest);
  };
  cleanups.push(() => (global.setTimeout = realSetTimeout));
  let tries = 0;
  const manager = createDaemonManager({
    connectors: {
      local: {
        kind: "bad",
        connect: async () => {
          tries += 1;
          throw Object.assign(new Error("no daemon"), {
            reason: "incompatible",
          });
        },
      },
    },
    random: () => 1,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(() => tries >= 8, 3000, "retries");
  global.setTimeout = realSetTimeout;
  assert.deepEqual(
    delays.slice(0, 8),
    [250, 500, 1000, 2000, 4000, 8000, 16000, 30000],
  );
  assert.deepEqual(stateOf(manager), {
    host: "local",
    state: "failed",
    generation: 0,
    reason: "incompatible",
    message: "no daemon",
  });
});

test("power resume retries a waiting host at once", async () => {
  const power = new EventEmitter();
  let tries = 0;
  const manager = startManager(
    {
      kind: "slow",
      connect: async () => {
        tries += 1;
        throw new Error("down");
      },
    },
    { powerMonitor: power, backoffMinMs: 60000, backoffMaxMs: 60000 },
  );
  manager.start();
  await until(() => tries === 1);
  await until(() => stateOf(manager).state === "failed");
  power.emit("resume");
  await until(() => tries === 2, 1000, "retry after resume");
});

// local.cjs ---------------------------------------------------------------

// A "sushiai" binary: --version prints the bundled version, `hooks install` is logged.
function fakeBinary(dir, version) {
  const file = path.join(dir, "fake-sushiai");
  fs.writeFileSync(
    file,
    `#!/bin/sh\ncase "$1" in\n  --version) echo "sushiai ${version}";;\n  hooks) echo "$*" >> "${dir}/hooks.log";;\nesac\n`,
    { mode: 0o755 },
  );
  return file;
}

const sha256 = (file) =>
  require("node:crypto")
    .createHash("sha256")
    .update(fs.readFileSync(file))
    .digest("hex");
function localFixture({ bundled = "1.0.0", appVersion = "app-1" } = {}) {
  const dir = tmp();
  const home = path.join(dir, "home");
  const binary = fakeBinary(dir, bundled);
  const socketPath = path.join(home, "daemon.sock");
  const spawned = [];
  const env = {
    HOME: dir,
    PATH: process.env.PATH,
    SUSHIAI_HOME: home,
    SUSHIAI_DAEMON_BIN: binary,
    SECRET_APP_VAR: "x",
  };
  const options = (extra = {}) => ({
    env,
    appVersion,
    pollMs: 10,
    ...extra,
  });
  // A daemon of the fixture's binary: same version, same build.
  const current = { version: bundled, build: sha256(binary) };
  return { dir, home, binary, socketPath, spawned, env, options, current };
}

test("local connector creates the home 0700 and starts a missing daemon detached", async () => {
  const f = localFixture();
  let daemon;
  const connector = createLocalConnector(
    f.options({
      spawn: (bin, args, opts) => {
        f.spawned.push({ bin, args, opts });
        fakeDaemon(f.socketPath, f.current).then((d) => (daemon = d));
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  const client = await connector.connect();
  cleanups.push(() => client.close());
  assert.equal(fs.statSync(f.home).mode & 0o777, 0o700);
  assert.equal(client.hello.daemon, "1.0.0");
  assert.equal(f.spawned.length, 1);
  assert.equal(f.spawned[0].bin, f.binary);
  assert.deepEqual(f.spawned[0].args, ["daemon"]);
  assert.equal(f.spawned[0].opts.detached, true);
  assert.equal(f.spawned[0].opts.env.SUSHIAI_HOME, f.home);
  assert.equal(f.spawned[0].opts.env.SECRET_APP_VAR, undefined);
  assert.equal(fs.existsSync(path.join(f.home, "daemon.log")), true);
  assert.equal(fs.existsSync(path.join(f.home, "bin")), false);
  assert.equal(fs.existsSync(path.join(f.home, "daemon-binary")), false);
  assert.ok(daemon);
  // A daemon that is already up and current is reused, not spawned again.
  const again = await connector.connect();
  again.close();
  assert.equal(f.spawned.length, 1);
});

test("a daemon of another version is shut down and the bundled one started", async () => {
  const f = localFixture({ bundled: "2.0.0" });
  fs.mkdirSync(f.home, { mode: 0o700 });
  const old = await fakeDaemon(f.socketPath, { version: "1.0.0" });
  const fresh = [];
  const connector = createLocalConnector(
    f.options({
      spawn: (bin, args) => {
        f.spawned.push(args);
        // The old socket is gone by now: the new daemon binds the same path.
        assert.equal(fs.existsSync(f.socketPath), false);
        fakeDaemon(f.socketPath, f.current).then((d) => fresh.push(d));
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  const client = await connector.connect();
  cleanups.push(() => client.close());
  assert.equal(client.hello.daemon, "2.0.0");
  assert.ok(old.log.some((m) => m.method === "daemon.shutdown"));
  assert.deepEqual(f.spawned, [["daemon"]]);
});

test("a binary that stays on the wrong version is reported incompatible", async () => {
  const f = localFixture({ bundled: "3.0.0" });
  fs.mkdirSync(f.home, { mode: 0o700 });
  await fakeDaemon(f.socketPath, { version: "1.0.0" });
  const connector = createLocalConnector(
    f.options({
      spawn: () => {
        fakeDaemon(f.socketPath, { version: "1.0.0" });
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  await assert.rejects(
    connector.connect(),
    (error) => error.reason === "incompatible",
  );
});

test("hooks install runs once per app version", async () => {
  const f = localFixture({ appVersion: "app-1" });
  fs.mkdirSync(f.home, { mode: 0o700 });
  await fakeDaemon(f.socketPath, f.current);
  const hooksLog = () => {
    try {
      return fs
        .readFileSync(path.join(f.dir, "hooks.log"), "utf8")
        .trim()
        .split("\n");
    } catch {
      return [];
    }
  };
  for (let i = 0; i < 2; i++) {
    const client = await createLocalConnector(f.options()).connect();
    client.close();
  }
  assert.deepEqual(hooksLog(), ["hooks install"]);
  assert.equal(
    fs.readFileSync(path.join(f.home, HOOKS_STAMP), "utf8").trim(),
    "app-1",
  );
  const next = await createLocalConnector(
    f.options({ appVersion: "app-2" }),
  ).connect();
  next.close();
  assert.equal(hooksLog().length, 2);
});

test("a daemon of another build is replaced once per app run, then reported incompatible", async () => {
  const f = localFixture();
  fs.mkdirSync(f.home, { mode: 0o700 });
  // Same version, another binary: only hello.build tells them apart.
  const old = await fakeDaemon(f.socketPath, {
    version: "1.0.0",
    build: "0".repeat(64),
  });
  let started = 0;
  const connector = createLocalConnector(
    f.options({
      spawn: () => {
        started++;
        fakeDaemon(f.socketPath, f.current);
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  const client = await connector.connect();
  assert.equal(started, 1);
  assert.ok(old.log.some((m) => m.method === "daemon.shutdown"));
  client.close();
  // Another app replaced the daemon with its own binary: no second shutdown.
  await new Promise((resolve) => setTimeout(resolve, 100));
  const other = await fakeDaemon(f.socketPath, { build: "1".repeat(64) });
  await assert.rejects(
    connector.connect(),
    (error) =>
      error.reason === "incompatible" &&
      error.retry === false &&
      /Another sushiAI build owns the daemon/.test(error.message),
  );
  assert.equal(started, 1);
  assert.equal(
    other.log.some((m) => m.method === "daemon.shutdown"),
    false,
  );
});

test("a daemon that reports no build (an older daemon) is not current", async () => {
  const f = localFixture();
  fs.mkdirSync(f.home, { mode: 0o700 });
  const old = await fakeDaemon(f.socketPath, { version: "1.0.0" });
  let started = 0;
  const connector = createLocalConnector(
    f.options({
      spawn: () => {
        started++;
        fakeDaemon(f.socketPath, f.current);
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  (await connector.connect()).close();
  assert.equal(started, 1);
  assert.ok(old.log.some((m) => m.method === "daemon.shutdown"));
});

test("a daemon started by this call that is not the bundled build is incompatible, not replaced", async () => {
  const f = localFixture();
  fs.mkdirSync(f.home, { mode: 0o700 });
  // Another app won the race for the socket with its own binary.
  const connector = createLocalConnector(
    f.options({
      spawn: () => {
        fakeDaemon(f.socketPath, { build: "2".repeat(64) });
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  await assert.rejects(
    connector.connect(),
    (error) => error.reason === "incompatible" && error.retry === false,
  );
});

test("restart stops the daemon of another build so the next connect starts this app's", async () => {
  const f = localFixture();
  fs.mkdirSync(f.home, { mode: 0o700 });
  const first = await fakeDaemon(f.socketPath, { build: "3".repeat(64) });
  let started = 0;
  const connector = createLocalConnector(
    f.options({
      spawn: () => {
        started++;
        fakeDaemon(f.socketPath, f.current);
        return Object.assign(new EventEmitter(), { unref() {} });
      },
    }),
  );
  // The one allowed replacement is used up; the next mismatch is incompatible.
  (await connector.connect()).close();
  assert.ok(first.log.some((m) => m.method === "daemon.shutdown"));
  await new Promise((resolve) => setTimeout(resolve, 100));
  const other = await fakeDaemon(f.socketPath, { build: "4".repeat(64) });
  await assert.rejects(
    connector.connect(),
    (error) => error.reason === "incompatible",
  );
  await connector.restart();
  assert.ok(other.log.some((m) => m.method === "daemon.shutdown"));
  const client = await connector.connect();
  cleanups.push(() => client.close());
  assert.equal(client.hello.build, f.current.build);
  assert.equal(started, 2);
});

test("a test run never runs hooks install", async () => {
  const f = localFixture();
  fs.mkdirSync(f.home, { mode: 0o700 });
  await fakeDaemon(f.socketPath, f.current);
  const vars = { ...f.env, SUSHIAI_TEST_WINDOW: "hidden" };
  (await createLocalConnector(f.options({ env: vars })).connect()).close();
  assert.equal(fs.existsSync(path.join(f.dir, "hooks.log")), false);
  assert.equal(fs.existsSync(path.join(f.home, HOOKS_STAMP)), false);
});

test("a packaged app without its bundled daemon fails with a clear error", () => {
  assert.throws(
    () =>
      resolveBinary({
        env: {},
        isPackaged: true,
        resourcesPath: "/r/Resources",
        exists: () => false,
      }),
    /missing its bundled daemon.*\/r\/Resources\/sushiai/,
  );
  assert.equal(
    resolveBinary({
      env: {},
      isPackaged: true,
      resourcesPath: "/r/Resources",
      exists: () => true,
    }),
    "/r/Resources/sushiai",
  );
});

test("the daemon environment keeps LC_CTYPE and defaults the locale to UTF-8", async () => {
  for (const [env, expect] of [
    [{}, { LANG: "en_US.UTF-8" }],
    [
      { LC_ALL: "de_DE.UTF-8", LC_CTYPE: "de_DE.UTF-8" },
      { LC_ALL: "de_DE.UTF-8", LC_CTYPE: "de_DE.UTF-8", LANG: undefined },
    ],
    [{ LANG: "fr_FR.UTF-8" }, { LANG: "fr_FR.UTF-8" }],
  ]) {
    const f = localFixture();
    const spawned = [];
    const connector = createLocalConnector(
      f.options({
        env: { ...f.env, ...env },
        spawn: (bin, args, opts) => {
          spawned.push(opts.env);
          fakeDaemon(f.socketPath, f.current);
          return Object.assign(new EventEmitter(), { unref() {} });
        },
      }),
    );
    (await connector.connect()).close();
    for (const [key, value] of Object.entries(expect))
      assert.equal(spawned[0][key], value, key);
  }
});

test("a symlinked home is refused with a message that names the path", () => {
  const dir = tmp();
  const real = path.join(dir, "real");
  fs.mkdirSync(real);
  const link = path.join(dir, "link");
  fs.symlinkSync(real, link);
  assert.throws(
    () => ensureHome(link),
    (error) => error.message.includes(link) && /0700/.test(error.message),
  );
});

test("in a checkout the newer of the release and debug binaries is used", () => {
  const times = {
    "/r/target/release/sushiai": 5,
    "/r/target/debug/sushiai": 9,
  };
  const pick = (extra) =>
    resolveBinary({
      env: {},
      repoRoot: "/r",
      exists: (file) => file in times,
      mtimeOf: (file) => times[file],
      ...extra,
    });
  assert.equal(pick(), "/r/target/debug/sushiai");
  times["/r/target/release/sushiai"] = 20;
  assert.equal(pick(), "/r/target/release/sushiai");
});

// Real daemon -------------------------------------------------------------

function realBinary() {
  const candidates = [
    process.env.SUSHIAI_DAEMON_BIN,
    process.env.CARGO_TARGET_DIR &&
      path.join(process.env.CARGO_TARGET_DIR, "debug", "sushiai"),
    path.join(__dirname, "..", "target", "debug", "sushiai"),
    path.join(__dirname, "..", "target", "release", "sushiai"),
  ];
  return candidates.find((file) => file && fs.existsSync(file));
}

test("real daemon: kill -9 and the manager is ready again with the session still listed", async (t) => {
  const binary = realBinary();
  if (!binary) return t.skip("sushiai binary not built");
  const dir = tmp();
  const home = path.join(dir, "h");
  const env = {
    HOME: dir,
    CODEX_HOME: path.join(dir, "codex"),
    PATH: process.env.PATH,
    SUSHIAI_HOME: home,
    SUSHIAI_DAEMON_BIN: binary,
  };
  const manager = startManager(
    createLocalConnector({ env, appVersion: "test" }),
    {
      backoffMinMs: 100,
    },
  );
  let session = null;
  cleanups.push(async () => {
    // Stop everything this test started: the session's holder, then the daemon,
    // and wait until both processes are gone so the temp dir can be removed.
    if (manager.states()[0].state !== "ready") return;
    const alive = (pid) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch {
        return false;
      }
    };
    const pids = [];
    const lock = fs.readFileSync(path.join(home, "daemon.lock"), "utf8");
    pids.push(Number.parseInt(lock.trim(), 10));
    const listed = await manager.request("local", "session.list", {});
    for (const entry of listed) if (entry.holderPid) pids.push(entry.holderPid);
    if (session)
      await manager
        .request("local", "session.close", { id: session.id, graceful: false })
        .catch(() => {});
    await manager.request("local", "daemon.shutdown", {}).catch(() => {});
    await until(() => !pids.some(alive), 10000, "daemon and holder to stop");
  });
  manager.start();
  await until(() => stateOf(manager).state === "ready", 20000, "first ready");
  assert.equal(stateOf(manager).generation, 1);
  assert.equal(fs.statSync(home).mode & 0o777, 0o700);

  session = await manager.request("local", "session.create", {
    cmd: ["/bin/sh"],
    cwd: dir,
    cols: 80,
    rows: 24,
  });
  const lock = fs.readFileSync(path.join(home, "daemon.lock"), "utf8");
  const pid = Number.parseInt(lock.trim(), 10);
  assert.ok(pid > 1);
  const started = Date.now();
  process.kill(pid, "SIGKILL");
  await until(
    () =>
      stateOf(manager).generation === 2 && stateOf(manager).state === "ready",
    5000,
    "ready after kill -9",
  );
  assert.ok(Date.now() - started < 5000);
  const listed = await manager.request("local", "session.list", {});
  assert.ok(listed.some((entry) => entry.id === session.id));
});

test("request forwards a per-call timeout to the client and none by default", async () => {
  const calls = [];
  const client = new EventEmitter();
  client.hello = { daemon: "1", host: "h", capabilities: [] };
  client.request = async (method, params, options) => {
    calls.push({ method, params, options });
    return {};
  };
  client.close = () => {};
  const manager = createDaemonManager({
    connectors: { h: { kind: "ssh", connect: async () => client } },
    backoffMinMs: 5,
  });
  cleanups.push(() => manager.close());
  manager.start();
  await until(
    () => manager.states().every((state) => state.state === "ready"),
    3000,
    "ready",
  );
  calls.length = 0;
  await manager.request("h", "a.long", { x: 1 }, { timeoutMs: 600000 });
  await manager.request("h", "a.short", {});
  assert.deepEqual(calls, [
    { method: "a.long", params: { x: 1 }, options: { timeoutMs: 600000 } },
    { method: "a.short", params: {}, options: undefined },
  ]);
});
