const { test } = require("node:test");
const assert = require("node:assert/strict");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const {
  OrchestratorService,
  socketPathFor,
  orchdBinaryPath,
  isStalePing,
  waitForExit,
  ALLOWED_METHODS,
  NOT_BUILT,
  FAILED_TO_START,
} = require("../electron/orchestrator.cjs");

async function waitUntil(check, { timeout = 2000, interval = 5 } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error("waitUntil: condition never became true");
}

/** A tiny NDJSON-RPC server standing in for `orchd`: `handlers[method]`
 * returns (or throws) the result for one request. Writes `control.token`
 * into its own data dir up front, same file/shape the real daemon writes, so
 * the service under test has something to authenticate with. A `subscribe`
 * request gets no reply and instead marks its socket as the one `pushEvent`
 * writes raw event lines to - the real daemon never answers it either. */
async function fixtureServer(t, handlers, { token = "test-token" } = {}) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "orchd-test-"));
  const socketPath = path.join(directory, "orchd.sock");
  await fs.writeFile(path.join(directory, "control.token"), token, {
    mode: 0o600,
  });
  const calls = [];
  const connections = new Set();
  let subscribeSocket = null;
  const server = net.createServer((socket) => {
    connections.add(socket);
    socket.on("close", () => {
      connections.delete(socket);
      if (subscribeSocket === socket) subscribeSocket = null;
    });
    let buffer = "";
    socket.on("data", async (chunk) => {
      buffer += chunk;
      let boundary;
      while ((boundary = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 1);
        if (!line.trim()) continue;
        const message = JSON.parse(line);
        calls.push({
          method: message.method,
          params: message.params,
          auth: message.auth,
        });
        if (message.method === "subscribe") {
          subscribeSocket = socket;
          continue;
        }
        try {
          const result = await handlers[message.method]?.(message.params);
          socket.write(
            JSON.stringify({ id: message.id, result: result ?? {} }) + "\n",
          );
        } catch (error) {
          socket.write(
            JSON.stringify({
              id: message.id,
              error: { message: error.message },
            }) + "\n",
          );
        }
      }
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    connections.forEach((socket) => socket.destroy());
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  });
  return {
    socketPath,
    directory,
    calls,
    token,
    hasSubscriber: () => Boolean(subscribeSocket),
    pushEvent: (event) => subscribeSocket?.write(JSON.stringify(event) + "\n"),
  };
}

/** A service wired to a running fixture server so `#ensureRunning` never
 * needs to spawn - `binary` just has to exist. */
async function serviceAgainst(t, socketPath, directory, extra = {}) {
  const binary = path.join(directory, "orchd");
  await fs.writeFile(binary, "#!/bin/sh\n", { mode: 0o755 });
  const service = new OrchestratorService({
    dataDir: directory,
    root: directory,
    resourcesPath: directory,
    packaged: false,
    send: () => {},
    ...extra,
  });
  service.binary = binary;
  service.socketPath = socketPath;
  t.after(() => service.close());
  return service;
}

test("socketPathFor keeps a short data dir, falls back for a long one", () => {
  const short = "/Users/sushi/Library/Application Support/sushiAI/orchestrator";
  assert.equal(socketPathFor(short), path.join(short, "orchd.sock"));
  const long = "/Users/sushi/" + "x".repeat(120) + "/orchestrator";
  const fallback = socketPathFor(long, "/tmp");
  assert.equal(path.dirname(fallback), "/tmp");
  assert.match(path.basename(fallback), /^sushi-orchd-[0-9a-f]{8}\.sock$/);
  // Deterministic per data dir, so a restart reuses the same fallback path.
  assert.equal(fallback, socketPathFor(long, "/tmp"));
});

test("orchdBinaryPath resolves dev vs packaged locations", () => {
  assert.equal(
    orchdBinaryPath({ root: "/repo", packaged: false }),
    path.join("/repo", "orchd", "target", "release", "orchd"),
  );
  assert.equal(
    orchdBinaryPath({ resourcesPath: "/App/Resources", packaged: true }),
    path.join("/App/Resources", "orchd"),
  );
});

test("isStalePing: only an idle daemon whose binary is newer by more than a second counts as stale", () => {
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 0 }, 3000), true);
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 0 }, 1500), false);
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 2 }, 5000), false);
  assert.equal(isStalePing({}, 5000), false);
  assert.equal(isStalePing(null, 5000), false);
});

test("waitForExit polls until the pid is gone, and gives up quietly after its timeout", async () => {
  let attempts = 0;
  await waitForExit(4242, {
    killFn: () => {
      attempts++;
      if (attempts >= 3) throw new Error("ESRCH");
    },
    timeoutMs: 1000,
    intervalMs: 1,
  });
  assert.equal(attempts, 3);

  const start = Date.now();
  await waitForExit(4242, {
    killFn: () => {}, // always "alive"
    timeoutMs: 20,
    intervalMs: 5,
  });
  assert.ok(Date.now() - start >= 20, "waited out the full timeout");
});

test("call() rejects a method outside the protocol allowlist before touching the socket", async () => {
  const service = new OrchestratorService({
    dataDir: "/tmp/does-not-matter",
    root: "/tmp/does-not-matter",
    send: () => {},
  });
  for (const method of ["hook.stop", "shutdown", "subscribe", "rm -rf /"])
    await assert.rejects(
      service.call(method),
      /Invalid orchestrator request/,
      method,
    );
  // Every pre-existing method stays reachable.
  for (const method of [
    "ping",
    "settings.get",
    "settings.set",
    "task.list",
    "task.get",
    "task.create",
    "task.start",
    "task.stop",
    "task.answer",
    "task.delete",
    "task.archive",
    "task.unarchive",
    "task.preflight",
    "chat.get",
    "chat.send",
    "chat.cancel",
    "chat.list",
    "chat.new",
    "chat.switch",
    "chat.clear",
  ])
    assert.equal(ALLOWED_METHODS.has(method), true, method);
  // Agent-to-agent message threads are reachable the same way as tasks/chat.
  assert.equal(ALLOWED_METHODS.has("message.list"), true);
  assert.equal(ALLOWED_METHODS.has("hook.stop"), false);
});

test("call() reports the daemon as not built rather than hanging on a missing binary", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "orchd-missing-"));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const service = new OrchestratorService({
    dataDir: directory,
    root: directory,
    resourcesPath: directory,
    packaged: false,
    send: () => {},
  });
  await assert.rejects(service.call("ping"), new RegExp(NOT_BUILT));
});

test("call() reports a distinct error when a spawned daemon never answers ping (binary exists but is stuck)", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "orchd-stuck-"));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const binary = path.join(directory, "orchd");
  await fs.writeFile(binary, "#!/bin/sh\n", { mode: 0o755 });
  const service = new OrchestratorService({
    dataDir: directory,
    root: directory,
    resourcesPath: directory,
    packaged: false,
    send: () => {},
    spawnRetries: 3,
    spawnIntervalMs: 5,
  });
  service.binary = binary;
  service.socketPath = path.join(directory, "orchd.sock"); // nobody listens here
  await assert.rejects(service.call("ping"), new RegExp(FAILED_TO_START));
});

test("every request but ping carries the daemon's control token", async (t) => {
  const { socketPath, directory, calls, token } = await fixtureServer(t, {
    ping: () => ({}),
    "task.list": () => [],
  });
  const service = await serviceAgainst(t, socketPath, directory);
  await service.call("task.list", { repo: "/x" });
  assert.equal(calls.find((c) => c.method === "ping").auth, undefined);
  assert.equal(calls.find((c) => c.method === "task.list").auth, token);
});

test("task.create is enriched with the repo's enabled MCP launch config", async (t) => {
  const { socketPath, directory, calls, token } = await fixtureServer(t, {
    ping: () => ({}),
    "task.create": (params) => ({ id: "t1", ...params }),
  });
  const mcpSnapshot = { mcpServers: { fs: { command: "fs-server" } } };
  const service = await serviceAgainst(t, socketPath, directory, {
    getClaudeMcp: () => ({ launchConfig: async () => mcpSnapshot }),
  });
  const result = await service.call("task.create", {
    repo: "/Users/sushi/project",
    title: "Export CSV",
  });
  assert.equal(result.id, "t1");
  const sent = calls.find((c) => c.method === "task.create");
  assert.deepEqual(sent.params.mcp, mcpSnapshot);
  assert.equal(sent.params.title, "Export CSV");
  assert.equal(sent.auth, token);
});

test("task.create proceeds without mcp when the project has no readable MCP config", async (t) => {
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({}),
    "task.create": (params) => ({ id: "t2", ...params }),
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    getClaudeMcp: () => ({
      launchConfig: async () => {
        throw new Error("no such project");
      },
    }),
  });
  await service.call("task.create", { repo: "/nope", title: "x" });
  const sent = calls.find((c) => c.method === "task.create").params;
  assert.equal("mcp" in sent, false);
});

test("settings.set pushes a full-replace secrets.set: classifier key/baseUrl plus each route's resolved profile env", async (t) => {
  const settings = {
    routes: [
      { id: "r1", label: "Sonnet", harness: "claude", profileId: "prof-1" },
      { id: "r2", label: "Codex", harness: "codex" },
    ],
    classifier: { backend: "openrouter", model: "", providerId: "prov-1" },
  };
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({}),
    "settings.set": () => settings,
    "settings.get": () => settings,
    "secrets.set": () => ({}),
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    getModelProviders: () => ({
      keyFor: async (id) => (id === "prov-1" ? "sk-secret" : null),
      listProviders: async () => [
        {
          id: "prov-1",
          baseUrl: "https://openrouter.ai/api",
          label: "OpenRouter",
        },
      ],
      resolveEnv: async (id) => {
        if (id !== "prof-1") throw new Error("Model profile not found.");
        return { settings: { ANTHROPIC_MODEL: "x" }, key: "prof-key" };
      },
      // The old apiKeyHelper staging path must not be touched anymore.
      stageSettings: async () => {
        throw new Error("stageSettings should not be called");
      },
    }),
  });
  await service.call("settings.set", { settings });
  const secretsCall = calls.find((c) => c.method === "secrets.set");
  assert.ok(secretsCall, "secrets.set was pushed");
  assert.deepEqual(secretsCall.params, {
    classifier: { key: "sk-secret", baseUrl: "https://openrouter.ai/api" },
    profiles: { "prof-1": { env: { ANTHROPIC_MODEL: "x" }, key: "prof-key" } },
  });
});

test("secrets.set is always pushed, even to clear it: no provider or key means classifier:null, profiles:{}", async (t) => {
  const settings = {
    routes: [],
    classifier: { backend: "none", model: "", providerId: "" },
  };
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({}),
    "settings.set": () => settings,
    "settings.get": () => settings,
    "secrets.set": () => ({}),
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    getModelProviders: () => ({
      keyFor: async () => null,
      listProviders: async () => [],
      resolveEnv: async () => {
        throw new Error("should not be called");
      },
    }),
  });
  await service.call("settings.set", { settings });
  const secretsCall = calls.find((c) => c.method === "secrets.set");
  assert.ok(
    secretsCall,
    "a full replace is pushed even when everything clears",
  );
  assert.deepEqual(secretsCall.params, { classifier: null, profiles: {} });
});

test("connect() relays subscribe events and raises one attention notice per (task, question)", async (t) => {
  const { socketPath, directory, hasSubscriber, pushEvent } =
    await fixtureServer(t, {
      ping: () => ({}),
      "settings.get": () => ({ routes: [], classifier: { providerId: "" } }),
    });
  const relayed = [];
  const notices = [];
  const service = await serviceAgainst(t, socketPath, directory, {
    send: (channel, value) => relayed.push([channel, value]),
    notify: async (notice) => {
      notices.push(notice);
    },
  });
  service.connect();
  await waitUntil(hasSubscriber);

  const task = (question) => ({
    id: "t1",
    title: "Do the thing",
    repo: "/repo",
    status: "waiting",
    question: question ? { text: question, options: [] } : undefined,
  });
  pushEvent({ event: "task", task: task("Delete old keys?") });
  await waitUntil(() => notices.length > 0);
  assert.equal(relayed.length, 1);
  assert.deepEqual(relayed[0], [
    "orchestrator-event",
    { event: "task", task: task("Delete old keys?") },
  ]);
  assert.deepEqual(notices[0], {
    workspaceId: "/repo",
    panelId: "t1",
    title: "Do the thing",
    body: "Delete old keys?",
  });

  // The same task re-entering `waiting` with the same question never re-notifies.
  pushEvent({ event: "task", task: task("Delete old keys?") });
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(notices.length, 1);

  // A genuinely new question on the same task does raise a second notice.
  pushEvent({ event: "task", task: task("Something else?") });
  await waitUntil(() => notices.length === 2);
  assert.equal(notices[1].body, "Something else?");
});
