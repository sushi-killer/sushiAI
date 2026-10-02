const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const { once } = require("node:events");
const {
  HERDR_CONTRACT,
  releaseArtifact,
} = require("../electron/herdr-contract.cjs");
const {
  checkHerdrCompatibility,
  assertHerdrCompatibility,
} = require("../electron/herdr-compatibility.cjs");
const { installPinnedHerdr } = require("../electron/herdr-install.cjs");
const { HerdrEvents } = require("../electron/herdr-events.cjs");

function schema() {
  return {
    protocol: HERDR_CONTRACT.protocol,
    schema_version: HERDR_CONTRACT.schemaVersion,
    schemas: {
      request: {
        oneOf: HERDR_CONTRACT.requiredMethods.map((method) => ({
          properties: { method: { const: method } },
        })),
        $defs: {
          ...Object.fromEntries(
            HERDR_CONTRACT.launchEnvMethods.map((name) => [
              name,
              { properties: { env: { type: "object" } } },
            ]),
          ),
          Subscription: {
            oneOf: HERDR_CONTRACT.eventTypes.map((type) => ({
              properties: { type: { const: type } },
            })),
          },
        },
      },
    },
  };
}

function cliOutput(args, options = {}) {
  if (args[0] === "--version")
    return `herdr ${options.version || HERDR_CONTRACT.version}\n`;
  if (args[0] === "api") return JSON.stringify(options.schema || schema());
  return options.stream === false
    ? "Unsupported command"
    : "Usage: herdr terminal session control [OPTIONS] <TARGET>\n--cols <N>\n--rows <N>\n";
}

function compatibilityOptions(overrides = {}) {
  return {
    endpoint: "/tmp/isolated-herdr.sock",
    connections: { socket: async () => "/tmp/isolated-herdr.sock" },
    binary: "/tmp/isolated-herdr-cli",
    runCli: async (_, args) => cliOutput(args),
    rpc: async () => ({
      version: HERDR_CONTRACT.version,
      protocol: HERDR_CONTRACT.protocol,
    }),
    ...overrides,
  };
}

test("daemon and stream CLI are checked independently before attachment", async () => {
  const checked = [];
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      rpc: async () => ({ version: "0.7.0", protocol: 19 }),
      runCli: async (_, args) => {
        checked.push(args);
        return cliOutput(args);
      },
    }),
  );
  assert.equal(status.compatible, false);
  assert.equal(status.daemon.compatible, false);
  assert.equal(status.cli.compatible, true);
  assert.equal(checked.length, 3);
  await assert.rejects(
    assertHerdrCompatibility(
      compatibilityOptions({
        runCli: async (_, args) =>
          cliOutput(args, { version: "0.9.0", stream: false }),
      }),
    ),
    (error) => {
      assert.equal(error.code, "HERDR_INCOMPATIBLE");
      assert.equal(error.data.daemon.compatible, true);
      assert.equal(error.data.cli.stream, false);
      assert.match(error.message, /terminal session control/);
      return true;
    },
  );
});

test("another Herdr version is used when its schema still offers the contract", async () => {
  // 0.8.2 (protocol 20) and a later release both pass on capabilities alone.
  for (const [version, protocol] of [
    ["0.8.2", 20],
    ["0.10.0", HERDR_CONTRACT.protocol + 1],
  ]) {
    const status = await checkHerdrCompatibility(
      compatibilityOptions({
        rpc: async () => ({ version, protocol }),
        runCli: async (_, args) =>
          cliOutput(args, { version, schema: { ...schema(), protocol } }),
      }),
    );
    assert.equal(status.compatible, true, version);
    assert.deepEqual(status.issues, []);
  }
});

test("a terminal CLI that speaks another protocol than the daemon is not attached", async () => {
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      rpc: async () => ({ version: "0.8.2", protocol: 20 }),
    }),
  );
  assert.equal(status.daemon.compatible, true);
  assert.equal(status.cli.compatible, false);
  assert.equal(status.compatible, false);
  assert.match(status.issues.join(" "), /does not match the daemon/);
});

test("the owner's CLI is used when sushiAI's pinned CLI speaks another protocol than their daemon", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-fallback-"));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const managed = path.join(directory, HERDR_CONTRACT.version, "herdr");
  await fs.mkdir(path.dirname(managed), { recursive: true });
  await fs.writeFile(managed, "managed CLI", { mode: 0o700 });
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      connections: {
        socket: async () => "/tmp/isolated-herdr.sock",
        herdrInstallDirectory: directory,
      },
      rpc: async () => ({ version: "0.8.2", protocol: 20 }),
      runCli: async (binary, args) =>
        binary === managed
          ? cliOutput(args)
          : cliOutput(args, {
              version: "0.8.2",
              schema: { ...schema(), protocol: 20 },
            }),
    }),
  );
  assert.equal(status.compatible, true);
  assert.equal(status.cli.binary, "/tmp/isolated-herdr-cli");
});

test("an earlier sushiAI-installed CLI is used while its daemon still runs", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-earlier-"));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const earlier = path.join(directory, "0.8.2", "herdr");
  await fs.mkdir(path.dirname(earlier), { recursive: true });
  await fs.writeFile(earlier, "earlier CLI", { mode: 0o700 });
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      binary: undefined,
      connections: {
        socket: async () => "/tmp/isolated-herdr.sock",
        herdrInstallDirectory: directory,
      },
      rpc: async () => ({ version: "0.8.2", protocol: 20 }),
      runCli: async (binary, args) => {
        assert.equal(binary, earlier);
        return cliOutput(args, {
          version: "0.8.2",
          schema: { ...schema(), protocol: 20 },
        });
      },
    }),
  );
  assert.equal(status.compatible, true);
  assert.equal(status.cli.binary, earlier);
});

test("over SSH the host's own CLI is used when sushiAI's pinned CLI speaks another protocol", async () => {
  const managed = `/home/user/.local/share/sushiai/herdr/${HERDR_CONTRACT.version}/herdr`;
  const owner = "/home/user/.local/bin/herdr";
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      endpoint: "ssh:host-one",
      connections: {
        socket: async () => "/tmp/forwarded.sock",
        exec: async (_, command) => {
          if (command.includes("for f in")) return `${managed}\n`;
          if (command.includes("if [ -x")) return `${managed}\n`;
          if (command.includes("command -v")) return `${owner}\n`;
          const old = command.includes(`'${owner}'`);
          const options = old
            ? { version: "0.8.2", schema: { ...schema(), protocol: 20 } }
            : {};
          if (command.includes("'--version'"))
            return cliOutput(["--version"], options);
          if (command.includes("'api'")) return cliOutput(["api"], options);
          return cliOutput(["terminal"]);
        },
      },
      rpc: async () => ({ version: "0.8.2", protocol: 20 }),
      runCli: async () => {
        throw new Error("Local CLI must not be used for SSH");
      },
    }),
  );
  assert.equal(status.compatible, true);
  assert.equal(status.cli.binary, owner);
});

test("CLI schema checks required methods, launch env and event capabilities", async () => {
  const incomplete = schema();
  incomplete.schemas.request.oneOf.pop();
  delete incomplete.schemas.request.$defs.PaneSplitParams.properties.env;
  incomplete.schemas.request.$defs.Subscription.oneOf.pop();
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      runCli: async (_, args) => cliOutput(args, { schema: incomplete }),
    }),
  );
  assert.equal(status.compatible, false);
  assert.ok(status.issues.some((issue) => issue.includes("events.subscribe")));
  assert.ok(
    status.issues.some((issue) => issue.includes("PaneSplitParams.env")),
  );
  assert.ok(status.issues.some((issue) => issue.includes("layout.updated")));
});

test("SSH probe uses the host CLI path independently of the tunnel daemon", async () => {
  const calls = [];
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      endpoint: "ssh:host-one",
      connections: {
        socket: async () => "/tmp/forwarded.sock",
        exec: async (endpoint, command) => {
          calls.push({ endpoint, command });
          if (command.includes("command -v")) return "/opt/host tools/herdr\n";
          if (command.includes("'--version'")) return cliOutput(["--version"]);
          if (command.includes("'api'")) return cliOutput(["api"]);
          return cliOutput(["terminal"]);
        },
      },
      runCli: async () => {
        throw new Error("Local CLI must not be used for SSH");
      },
    }),
  );
  assert.equal(status.compatible, true);
  assert.equal(status.daemon.socket, "/tmp/forwarded.sock");
  assert.equal(status.cli.binary, "/opt/host tools/herdr");
  assert.equal(calls.length, 4);
  assert.ok(
    calls
      .slice(1)
      .every(({ command }) => command.includes("'/opt/host tools/herdr'")),
  );
});

test("a daemon failure still returns the independently verified CLI status", async () => {
  const status = await checkHerdrCompatibility(
    compatibilityOptions({
      rpc: async () => {
        throw Object.assign(new Error("Socket gone"), { code: "ENOENT" });
      },
    }),
  );
  assert.equal(status.compatible, false);
  assert.equal(status.daemon.error.code, "ENOENT");
  assert.equal(status.cli.compatible, true);
});

test("explicit user CLI checks bypass managed preference locally and on SSH", async (t) => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "herdr-user-check-"),
  );
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const managed = path.join(directory, HERDR_CONTRACT.version, "herdr");
  await fs.mkdir(path.dirname(managed), { recursive: true });
  await fs.writeFile(managed, "managed CLI", { mode: 0o700 });
  const checked = [];
  const local = await checkHerdrCompatibility(
    compatibilityOptions({
      preferManaged: false,
      connections: {
        socket: async () => "/tmp/isolated-herdr.sock",
        herdrInstallDirectory: directory,
      },
      runCli: async (binary, args) => {
        checked.push(binary);
        return cliOutput(args, {
          version: "0.0.1",
          schema: { ...schema(), protocol: 19 },
        });
      },
    }),
  );
  assert.equal(local.cli.binary, "/tmp/isolated-herdr-cli");
  assert.equal(local.cli.compatible, false);
  assert.ok(checked.every((binary) => binary === "/tmp/isolated-herdr-cli"));
  const managedStatus = await checkHerdrCompatibility(
    compatibilityOptions({
      connections: {
        socket: async () => "/tmp/isolated-herdr.sock",
        herdrInstallDirectory: directory,
      },
      runCli: async (binary, args) => {
        assert.equal(binary, managed);
        return cliOutput(args);
      },
    }),
  );
  assert.equal(managedStatus.cli.binary, managed);
  assert.equal(managedStatus.compatible, true);
  const remoteCommands = [];
  const remote = await checkHerdrCompatibility(
    compatibilityOptions({
      endpoint: "ssh:user-host",
      binary: "/opt/user herdr",
      preferManaged: false,
      connections: {
        socket: async () => "/tmp/user-host.sock",
        exec: async (_, command) => {
          remoteCommands.push(command);
          return cliOutput(
            command.includes("'--version'")
              ? ["--version"]
              : command.includes("'api'")
                ? ["api"]
                : ["terminal"],
            { version: "0.0.1", schema: { ...schema(), protocol: 19 } },
          );
        },
      },
    }),
  );
  assert.equal(remote.cli.binary, "/opt/user herdr");
  assert.equal(remote.cli.compatible, false);
  assert.equal(remoteCommands.length, 3);
  assert.ok(
    remoteCommands.every(
      (command) =>
        command.includes("'/opt/user herdr'") &&
        !command.includes("command -v"),
    ),
  );
});

test("managed install rejects incorrect downloads and preserves an existing binary", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-install-"));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const artifact = releaseArtifact("linux", "x64");
  let fetched;
  await assert.rejects(
    installPinnedHerdr(directory, {
      platform: "linux",
      arch: "x64",
      fetchAsset: async (url) => {
        fetched = url;
        return { ok: true, body: [Buffer.from("incorrect downloaded bytes")] };
      },
    }),
    { code: "HERDR_CHECKSUM_MISMATCH" },
  );
  assert.equal(fetched, artifact.url);
  const destination = path.join(directory, HERDR_CONTRACT.version, "herdr");
  await assert.rejects(fs.stat(destination), { code: "ENOENT" });
  await fs.mkdir(path.dirname(destination), { recursive: true });
  await fs.writeFile(destination, "existing user bytes");
  await assert.rejects(
    installPinnedHerdr(directory, {
      platform: "linux",
      arch: "x64",
      fetchAsset: async () => {
        throw new Error("Must not download over existing binary");
      },
    }),
    { code: "HERDR_CHECKSUM_MISMATCH" },
  );
  assert.equal(await fs.readFile(destination, "utf8"), "existing user bytes");
});

async function eventFixture(t, respond) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-events-"));
  const socketPath = path.join(directory, "api.sock");
  const sockets = new Set();
  const requests = [];
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk;
      if (!buffer.includes("\n")) return;
      const message = JSON.parse(buffer.trim());
      buffer = "";
      requests.push(message);
      respond(socket, message, requests.length);
    });
  });
  server.listen(socketPath);
  await once(server, "listening");
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  });
  return { socketPath, sockets, requests };
}

function eventWaiter() {
  const events = [];
  const waiters = [];
  return {
    events,
    send: (_, event) => {
      events.push(event);
      for (const waiter of [...waiters]) {
        if (waiter.predicate(event)) {
          clearTimeout(waiter.timer);
          waiters.splice(waiters.indexOf(waiter), 1);
          waiter.resolve(event);
        }
      }
    },
    wait: (predicate) => {
      const existing = events.find(predicate);
      if (existing) return Promise.resolve(existing);
      return new Promise((resolve, reject) => {
        const waiter = {
          predicate,
          resolve,
          timer: setTimeout(
            () => reject(new Error("Event was not delivered")),
            3000,
          ),
        };
        waiters.push(waiter);
      });
    },
  };
}

test("one native subscription per endpoint handles fragmented events and reconnects with a new generation", async (t) => {
  const fixture = await eventFixture(t, (socket, request) => {
    assert.equal(request.method, "events.subscribe");
    assert.deepEqual(
      request.params.subscriptions,
      HERDR_CONTRACT.eventTypes.map((type) => ({ type })),
    );
    socket.write(
      JSON.stringify({
        id: request.id,
        result: { type: "subscription_started" },
      }) + "\n",
    );
    const frame = Buffer.from(
      JSON.stringify({
        event: "pane_created",
        data: { label: "Workspace 🍣" },
      }) + "\n",
    );
    const boundary = frame.indexOf(Buffer.from("🍣")) + 1;
    socket.write(frame.subarray(0, boundary));
    setImmediate(() => socket.write(frame.subarray(boundary)));
  });
  const waiter = eventWaiter();
  const manager = new HerdrEvents({
    getConnections: () => ({ socket: async () => fixture.socketPath }),
    send: waiter.send,
    reconnectDelay: 10,
  });
  t.after(() => manager.close());
  manager.subscribe(fixture.socketPath, "view-one");
  manager.subscribe(fixture.socketPath, "view-two");
  const changed = await waiter.wait((event) => event.type === "changed");
  assert.equal(changed.event, "pane_created");
  assert.equal(fixture.requests.length, 1);
  manager.unsubscribe(fixture.socketPath, "view-one");
  assert.equal(fixture.sockets.size, 1);
  for (const socket of fixture.sockets) socket.destroy();
  const reconnected = await waiter.wait(
    (event) =>
      event.type === "connected" && event.generation > changed.generation,
  );
  assert.ok(reconnected.generation > changed.generation);
  assert.equal(fixture.requests.length, 2);
  manager.unsubscribe(fixture.socketPath, "view-two");
  const count = waiter.events.length;
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(waiter.events.length, count);
  assert.equal(manager.endpoints.size, 0);
});

test("unsubscribe before SSH resolution prevents an obsolete subscription socket", async () => {
  let resolveSocket;
  const waiter = eventWaiter();
  const manager = new HerdrEvents({
    getConnections: () => ({
      socket: () =>
        new Promise((resolve) => {
          resolveSocket = resolve;
        }),
    }),
    send: waiter.send,
  });
  manager.subscribe("ssh:isolated", "view-one");
  manager.unsubscribe("ssh:isolated", "view-one");
  resolveSocket("/tmp/nonexistent-event-socket");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(waiter.events.length, 0);
  assert.equal(manager.endpoints.size, 0);
  manager.close();
});

test("malformed native frames reconnect without inventing successful state", async (t) => {
  const fixture = await eventFixture(t, (socket, request, count) => {
    socket.write(
      JSON.stringify({
        id: request.id,
        result: { type: "subscription_started" },
      }) + "\n",
    );
    if (count === 1) socket.write("invalid JSON\n");
  });
  const waiter = eventWaiter();
  const manager = new HerdrEvents({
    getConnections: () => ({ socket: async () => fixture.socketPath }),
    send: waiter.send,
    reconnectDelay: 10,
  });
  t.after(() => manager.close());
  manager.subscribe(fixture.socketPath, "view");
  const disconnected = await waiter.wait(
    (event) => event.type === "disconnected",
  );
  assert.equal(disconnected.error.code, "HERDR_INVALID_RESPONSE");
  await waiter.wait(
    (event) =>
      event.type === "connected" && event.generation > disconnected.generation,
  );
  assert.equal(
    waiter.events.filter((event) => event.type === "changed").length,
    0,
  );
});

test("an old tunnel resolution cannot publish events after the connection changes", async (t) => {
  const acknowledge = (socket, request) =>
    socket.write(
      JSON.stringify({
        id: request.id,
        result: { type: "subscription_started" },
      }) + "\n",
    );
  const previous = await eventFixture(t, acknowledge);
  const current = await eventFixture(t, acknowledge);
  let resolvePrevious,
    connectionChanged,
    attempts = 0,
    removedListener = false;
  const waiter = eventWaiter();
  const manager = new HerdrEvents({
    getConnections: () => ({
      socket: () =>
        ++attempts === 1
          ? new Promise((resolve) => {
              resolvePrevious = resolve;
            })
          : Promise.resolve(current.socketPath),
      onStateChange: (listener) => {
        connectionChanged = listener;
        return () => {
          removedListener = true;
        };
      },
    }),
    send: waiter.send,
  });
  t.after(() => manager.close());
  manager.subscribe("ssh:isolated-host", "view");
  connectionChanged({
    endpoint: "ssh:isolated-host",
    connected: true,
    generation: 2,
  });
  await waiter.wait((event) => event.type === "connected");
  resolvePrevious(previous.socketPath);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(previous.requests.length, 0);
  assert.equal(current.requests.length, 1);
  assert.equal(
    waiter.events.filter((event) => event.type === "connected").length,
    1,
  );
  manager.unsubscribe("ssh:isolated-host", "view");
  assert.equal(removedListener, true);
});
