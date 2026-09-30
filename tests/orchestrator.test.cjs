const { test } = require("node:test");
const assert = require("node:assert/strict");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const {
  OrchestratorService,
  OrchestratorHosts,
  orchestratorNotice,
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

test("isStalePing: a rebuilt binary replaces an idle daemon at once and a busy one only after the deferral", () => {
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 0 }, 3000), true);
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 0 }, 1500), false);
  assert.equal(isStalePing({ binaryMtimeMs: 1000, running: 2 }, 5000), false);
  assert.equal(
    isStalePing({ binaryMtimeMs: 1000, running: 2 }, 5000, 29 * 60 * 1000),
    false,
  );
  assert.equal(
    isStalePing({ binaryMtimeMs: 1000, running: 2 }, 5000, 30 * 60 * 1000),
    true,
  );
  assert.equal(
    isStalePing({ binaryMtimeMs: 1000, running: 0, chatTurns: 1 }, 5000),
    false,
  );
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
    "task.timeline",
    "failures.catalogue",
    "costs.summary",
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
  assert.equal(ALLOWED_METHODS.has("message.send"), true);
  // Read-only built-in defaults, compared against the saved settings.
  assert.equal(ALLOWED_METHODS.has("settings.defaults"), true);
  for (const method of [
    "evolution.run",
    "evolution.list",
    "evolution.approve",
    "evolution.reject",
    "evolution.adopt",
  ])
    assert.equal(ALLOWED_METHODS.has(method), true, method);
  for (const method of [
    "repo.notes.list",
    "repo.notes.add",
    "repo.notes.remove",
  ])
    assert.equal(ALLOWED_METHODS.has(method), true, method);
  assert.equal(ALLOWED_METHODS.has("hook.stop"), false);
});

test("every method the renderer client calls is allowlisted in main", () => {
  const client = require("node:fs").readFileSync(
    require("node:path").join(__dirname, "../src/orchestrator/client.ts"),
    "utf8",
  );
  const called = [
    ...client.matchAll(/call<[^>]*>\(\s*"([a-z]+\.[A-Za-z.]+)"/g),
  ].map((match) => match[1]);
  assert.ok(called.length > 20, "client.ts call pattern changed");
  for (const method of called)
    assert.equal(ALLOWED_METHODS.has(method), true, method);
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

test("probe() pings the socket in use and never spawns or provisions a daemon", async (t) => {
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({ pid: 7 }),
  });
  // No binary: anything that went through #ensureRunning would fail NOT_BUILT.
  const service = new OrchestratorService({
    dataDir: directory,
    root: directory,
    resourcesPath: directory,
    packaged: false,
    send: () => {},
  });
  service.binary = path.join(directory, "missing-orchd");
  service.socketPath = socketPath;
  assert.deepEqual(await service.probe(), { pid: 7 });
  assert.deepEqual(
    calls.map((c) => c.method),
    ["ping"],
  );
  // A dead socket is a plain connection error, not a spawn attempt.
  service.socketPath = path.join(directory, "nobody.sock");
  await assert.rejects(service.probe(), (error) => {
    assert.doesNotMatch(error.message, new RegExp(NOT_BUILT));
    assert.doesNotMatch(error.message, new RegExp(FAILED_TO_START));
    return true;
  });

  // A remote service that has no connection yet never calls ensure().
  let ensured = 0;
  const remote = new OrchestratorService({
    host: "ssh:box",
    send: () => {},
    remote: {
      ensure: async () => {
        ensured++;
        return { socketPath, token: "t" };
      },
      reopen() {},
      close() {},
    },
  });
  await assert.rejects(remote.probe(), /not connected/);
  assert.equal(ensured, 0);
});

test("OrchestratorHosts.probe never enables or creates a remote host", async () => {
  let created = 0;
  let probed = 0;
  const hosts = new OrchestratorHosts({
    local: { probe: async () => ({ pid: 1 }) },
    connections: () => ({ get: () => ({}), list: () => [] }),
    createService: () => {
      created++;
      return { connect() {}, probe: async () => ({ pid: 2 }) };
    },
    onChange: () => {},
  });
  assert.deepEqual(await hosts.probe("local"), { pid: 1 });
  await assert.rejects(hosts.probe("ssh:box"), /not connected/);
  assert.equal(created, 0);
  assert.equal(
    hosts.list().find((h) => h.id === "ssh:box"),
    undefined,
  );
  // Once a call connected the host, the probe reaches its service.
  hosts.services.set("ssh:box", {
    probe: async () => {
      probed++;
      return { pid: 2 };
    },
  });
  assert.deepEqual(await hosts.probe("ssh:box"), { pid: 2 });
  assert.equal(probed, 1);
  await assert.rejects(hosts.probe(42), /Invalid orchestrator host/);
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

test("settings.set pushes a full-replace secrets.set: each route's resolved profile env", async (t) => {
  const settings = {
    routes: [
      { id: "r1", label: "Sonnet", harness: "claude", profileId: "prof-1" },
      { id: "r2", label: "Codex", harness: "codex" },
    ],
  };
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({}),
    "settings.set": () => settings,
    "settings.get": () => settings,
    "secrets.set": () => ({}),
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    getModelProviders: () => ({
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
    profiles: { "prof-1": { env: { ANTHROPIC_MODEL: "x" }, key: "prof-key" } },
  });
});

test("secrets.set is always pushed, even to clear it: no profiles means profiles:{}", async (t) => {
  const settings = { routes: [] };
  const { socketPath, directory, calls } = await fixtureServer(t, {
    ping: () => ({}),
    "settings.set": () => settings,
    "settings.get": () => settings,
    "secrets.set": () => ({}),
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    getModelProviders: () => ({
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
  assert.deepEqual(secretsCall.params, { profiles: {} });
});

test("connect() relays subscribe events and raises one notice per (task, question)", async (t) => {
  const { socketPath, directory, hasSubscriber, pushEvent } =
    await fixtureServer(t, {
      ping: () => ({}),
      "settings.get": () => ({ routes: [] }),
    });
  const relayed = [];
  const notices = [];
  const service = await serviceAgainst(t, socketPath, directory, {
    send: (channel, value) => relayed.push([channel, value]),
    notify: (notice) => {
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
    taskId: "t1",
    repo: "/repo",
    kind: "input",
    title: "Do the thing",
    body: "Delete old keys?",
    focus: "question",
    repoName: "repo",
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

test("the transition detector notifies done and failed once, only for live top-level tasks; a fresh report titles the done notice", async (t) => {
  const { socketPath, directory, hasSubscriber, pushEvent } =
    await fixtureServer(t, {
      ping: () => ({}),
      "settings.get": () => ({ routes: [] }),
    });
  const notices = [];
  const service = await serviceAgainst(t, socketPath, directory, {
    send: () => {},
    notify: (notice) => {
      notices.push(notice);
    },
  });
  service.connect();
  await waitUntil(hasSubscriber);

  const base = {
    title: "Ship it",
    repo: "/repo",
    branch: "task/ship",
    baseRef: "main",
    costUsd: 1.234,
    archived: false,
    attempts: [],
  };
  const done = {
    ...base,
    id: "d1",
    status: "done",
    landedSha: "0123456789abcdef",
    report: "# Ship it",
    reportAt: Date.now(),
  };
  pushEvent({ event: "task", task: done });
  pushEvent({ event: "task", task: { ...done, costUsd: 2 } });
  pushEvent({ event: "task", task: { ...base, id: "d2", status: "done" } });
  pushEvent({
    event: "task",
    task: { ...base, id: "d3", status: "done", report: "# Old", reportAt: 1 },
  });
  pushEvent({ event: "task", task: { ...base, id: "f1", status: "running" } });
  pushEvent({
    event: "task",
    task: {
      ...base,
      id: "f1",
      status: "failed",
      attempts: [{ n: 1, failure: { kind: "verify" } }],
    },
  });
  // Subtask and archived tasks raise nothing.
  pushEvent({
    event: "task",
    task: { ...base, id: "s1", status: "done", parent: "d1" },
  });
  pushEvent({
    event: "task",
    task: { ...base, id: "s2", status: "failed", parent: "d1" },
  });
  pushEvent({
    event: "task",
    task: { ...base, id: "a1", status: "done", archived: true },
  });
  // A waiting subtask still asks.
  pushEvent({
    event: "task",
    task: {
      ...base,
      id: "s3",
      status: "waiting",
      parent: "d1",
      question: { text: "Which?" },
    },
  });
  pushEvent({
    event: "task",
    task: {
      ...base,
      id: "a2",
      status: "waiting",
      archived: true,
      question: { text: "Archived?" },
    },
  });
  await waitUntil(() => notices.length >= 5);
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(
    notices.map((n) => [n.taskId, n.kind, n.title, n.body, n.focus]),
    [
      [
        "d1",
        "done",
        "Feature done: Ship it",
        "$1.23 · landed 01234567 on main",
        "report",
      ],
      ["d2", "done", "Ship it", "$1.23 · on task/ship, not landed", "report"],
      ["d3", "done", "Ship it", "$1.23 · on task/ship, not landed", "report"],
      ["f1", "failed", "Ship it", "verify failed · $1.23", "summary"],
      ["s3", "input", "Ship it", "Which?", "question"],
    ],
  );
});

test("the transition detector notifies once when a top-level task lands dirty, fails or is stopped by the engine, never for an owner stop", async (t) => {
  const { socketPath, directory, hasSubscriber, pushEvent } =
    await fixtureServer(t, {
      ping: () => ({}),
      "settings.get": () => ({ routes: [] }),
    });
  const notices = [];
  const service = await serviceAgainst(t, socketPath, directory, {
    send: () => {},
    notify: (notice) => {
      notices.push(notice);
    },
  });
  service.connect();
  await waitUntil(hasSubscriber);
  const base = {
    title: "Ship it",
    repo: "/repo",
    costUsd: 0,
    archived: false,
    attempts: [],
    decisions: [],
  };
  const running = (id, extra = {}) =>
    pushEvent({
      event: "task",
      task: { ...base, id, status: "running", ...extra },
    });
  for (const id of ["l1", "e1", "o1", "o2", "f1", "a1"]) running(id);
  const landing = { ...base, id: "l1", status: "landing" };
  pushEvent({ event: "task", task: landing });
  pushEvent({ event: "task", task: { ...landing, costUsd: 1 } });
  const engineStopped = {
    ...base,
    id: "e1",
    status: "stopped",
    decisions: ["Orchestrator: no subtasks are left"],
  };
  pushEvent({ event: "task", task: engineStopped });
  pushEvent({ event: "task", task: { ...engineStopped, costUsd: 2 } });
  const failed = { ...base, id: "f1", status: "failed" };
  pushEvent({ event: "task", task: failed });
  pushEvent({ event: "task", task: { ...failed, costUsd: 3 } });
  pushEvent({
    event: "task",
    task: { ...base, id: "o1", status: "stopped", decisions: ["Owner: stop"] },
  });
  pushEvent({
    event: "task",
    task: {
      ...base,
      id: "o2",
      status: "stopped",
      attempts: [{ n: 1, status: "interrupted" }],
    },
  });
  pushEvent({
    event: "task",
    task: { ...base, id: "a1", status: "landing", archived: true },
  });
  // Already landing when first seen: not a transition, no notice.
  pushEvent({ event: "task", task: { ...base, id: "n1", status: "landing" } });
  await waitUntil(() => notices.length >= 3);
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(
    notices.map((n) => [n.taskId, n.kind, n.body]),
    [
      ["l1", "landing", "Waiting for a clean checkout to land."],
      ["e1", "stopped", "Stopped on its own - needs a look."],
      ["f1", "failed", "failed · $0.00"],
    ],
  );
});

test("orchestratorNotice shapes input, done and failed notices and trims to the caps", () => {
  const task = {
    id: "t1",
    title: "T".repeat(500),
    repo: "/repo",
    branch: "task/x",
    costUsd: 0,
  };
  const input = orchestratorNotice({
    ...task,
    status: "waiting",
    question: { text: "q".repeat(2500) },
  });
  assert.equal(input.kind, "input");
  assert.equal(input.focus, "question");
  assert.equal(input.title.length, 120);
  assert.equal(input.body.length, 2000);
  assert.equal(input.options, undefined);
  const withOptions = orchestratorNotice({
    ...task,
    status: "waiting",
    question: {
      text: "q",
      options: ["Stop", "", 7, "b".repeat(500), "c", "d", "e", "f"],
    },
  });
  assert.equal(withOptions.options.length, 4);
  assert.equal(withOptions.options[0], "Stop");
  assert.equal(withOptions.options[1].length, 400);
  assert.equal(
    orchestratorNotice({ ...task, status: "waiting" }).body,
    "Needs your input.",
  );
  const done = orchestratorNotice({ ...task, status: "done" });
  assert.equal(done.body, "$0.00 · on task/x, not landed");
  assert.equal(done.focus, "report");
  assert.equal(done.title, "T".repeat(120));
  const fresh = orchestratorNotice({
    ...task,
    status: "done",
    report: "# r",
    reportAt: Date.now(),
  });
  assert.equal(fresh.title, `Feature done: ${"T".repeat(500)}`.slice(0, 120));
  const stale = orchestratorNotice({
    ...task,
    status: "done",
    report: "# r",
    reportAt: 1,
  });
  assert.equal(stale.title, "T".repeat(120));
  const failed = orchestratorNotice({
    ...task,
    status: "failed",
    costUsd: 0.5,
    attempts: [
      { n: 1, failure: { kind: "stall" } },
      { n: 2, failure: { kind: "budget" } },
    ],
  });
  assert.deepEqual(failed, {
    taskId: "t1",
    repo: "/repo",
    kind: "failed",
    title: "T".repeat(120),
    body: "over budget · $0.50",
    focus: "summary",
    repoName: "repo",
    costUsd: 0.5,
  });
});

test("a test-mode quit() sends an authenticated shutdown and kills a daemon that ignores it", async (t) => {
  const { spawn } = require("node:child_process");
  const child = spawn("sleep", ["60"], { stdio: "ignore" });
  t.after(() => child.kill("SIGKILL"));
  const exited = new Promise((resolve) => child.on("exit", resolve));
  const { socketPath, directory, calls, token } = await fixtureServer(t, {
    ping: () => ({ pid: child.pid }),
    shutdown: () => {},
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    stopDaemonOnQuit: true,
    quitTimeoutMs: 300,
  });
  await service.quit();
  const shutdown = calls.find((c) => c.method === "shutdown");
  assert.ok(shutdown, "quit() must send shutdown");
  assert.equal(shutdown.auth, token);
  await Promise.race([
    exited,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("daemon still alive")), 2000),
    ),
  ]);
});

test("a normal quit() leaves the daemon alone and only closes the subscribe socket", async (t) => {
  const { spawn } = require("node:child_process");
  const child = spawn("sleep", ["60"], { stdio: "ignore" });
  t.after(() => child.kill("SIGKILL"));
  const { socketPath, directory, calls, hasSubscriber } = await fixtureServer(
    t,
    { ping: () => ({ pid: child.pid }) },
  );
  const service = await serviceAgainst(t, socketPath, directory);
  service.connect();
  await waitUntil(hasSubscriber);
  await service.quit();
  await waitUntil(() => !hasSubscriber());
  assert.equal(
    calls.some((c) => c.method === "shutdown"),
    false,
  );
  assert.doesNotThrow(() => process.kill(child.pid, 0));
});

test("quit() during a slow first ping never lets a later spawn start a daemon", async (t) => {
  let release;
  const gate = new Promise((resolve) => (release = resolve));
  const { socketPath, directory } = await fixtureServer(t, {
    ping: async () => {
      await gate;
      throw new Error("no daemon");
    },
  });
  const service = await serviceAgainst(t, socketPath, directory, {
    stopDaemonOnQuit: true,
    quitTimeoutMs: 100,
  });
  const marker = path.join(directory, "spawned");
  await fs.writeFile(service.binary, `#!/bin/sh\ntouch "${marker}"\n`, {
    mode: 0o755,
  });
  const call = service.call("task.list", {}).catch(() => {});
  await new Promise((resolve) => setTimeout(resolve, 100));
  const quitting = service.quit();
  release();
  await quitting;
  await call;
  await new Promise((resolve) => setTimeout(resolve, 200));
  await assert.rejects(fs.access(marker));
});
