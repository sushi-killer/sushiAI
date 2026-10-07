const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  OrchestratorService,
  OrchestratorHosts,
  createOrchestratorHosts,
  orchestratorNotice,
  ALLOWED_METHODS,
} = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

async function waitUntil(check, { timeout = 2000, interval = 5 } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error("waitUntil: condition never became true");
}

const REMOTE = "ssh:devbox-id";
const connections = {
  get: () => ({ name: "Devbox" }),
  list: () => [{ id: "devbox-id", name: "Devbox" }],
  hasShell: () => true,
  exec: async () => "git=1\nclaude=1\nclaude_login=1\n",
};

/** A service for `host` over a fake daemon manager. */
function serviceFor(manager, extra = {}) {
  return new OrchestratorService({
    host: "local",
    getManager: () => manager,
    getConnections: () => connections,
    send: () => {},
    ...extra,
  });
}

/** A manager whose local and remote hosts are both ready with `orch`. */
function twoHosts(handlers = {}, onRetry) {
  return fakeDaemonManager({
    hosts: { local: {}, "devbox-id": {} },
    handlers,
    onRetry,
  });
}

test("call() rejects a method outside the protocol allowlist before touching the daemon", async () => {
  const manager = fakeDaemonManager();
  const service = serviceFor(manager);
  for (const method of [
    "hook.stop",
    "shutdown",
    "subscribe",
    "ping",
    "rm -rf /",
  ])
    await assert.rejects(
      service.call(method),
      /Invalid orchestrator request/,
      method,
    );
  assert.equal(manager.calls.length, 0);
  // Every pre-existing method stays reachable.
  for (const method of [
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
    "task.openDeliverable",
    "failures.catalogue",
    "costs.summary",
    "chat.get",
    "chat.send",
    "chat.edit",
    "chat.cancel",
    "chat.list",
    "chat.new",
    "chat.switch",
    "chat.clear",
    "message.list",
    "message.send",
    "settings.defaults",
    "evolution.run",
    "evolution.list",
    "evolution.approve",
    "evolution.reject",
    "evolution.adopt",
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

test("every request goes to the daemon as orch.<method> with its own timeout", async () => {
  const manager = fakeDaemonManager({
    handlers: { "orch.task.list": () => [{ id: "t1", status: "queued" }] },
  });
  const service = serviceFor(manager);
  const tasks = await service.call("task.list", { repo: "/x" });
  assert.deepEqual(tasks, [{ id: "t1", status: "queued" }]);
  await service.call("task.pr", { id: "t1" });
  assert.deepEqual(
    manager.calls.map((call) => [call.host, call.method, call.options]),
    [
      ["local", "orch.task.list", { timeoutMs: 20000 }],
      ["local", "orch.task.pr", { timeoutMs: 600000 }],
    ],
  );
  assert.deepEqual(manager.calls[0].params, { repo: "/x" });
});

test("a daemon without the orch capability answers Update sushiai on <host>, and nothing is sent", async () => {
  const manager = fakeDaemonManager({
    hosts: {
      local: { capabilities: ["sessions"] },
      "devbox-id": { capabilities: ["sessions", "attach"] },
    },
  });
  await assert.rejects(serviceFor(manager).call("task.list"), {
    message: "Update sushiai on this Mac",
    code: "ORCH_MISSING",
  });
  await assert.rejects(
    serviceFor(manager, { host: REMOTE }).call("task.list"),
    { message: "Update sushiai on Devbox", code: "ORCH_MISSING" },
  );
  assert.equal(manager.calls.length, 0);
  // The panel's host list shows the same instruction instead of "ready".
  const hosts = createOrchestratorHosts({
    send: () => {},
    getConnections: () => connections,
    getManager: () => manager,
    enabled: true,
  });
  const record = hosts.list().find((host) => host.id === REMOTE);
  assert.equal(record.state, "error");
  assert.equal(record.detail, "Update sushiai on Devbox");
});

test("without a daemon manager the call says the daemon is not running", async () => {
  const service = new OrchestratorService({
    getManager: () => null,
    send: () => {},
  });
  await assert.rejects(service.call("task.list"), {
    message: "The sushiai daemon is not running.",
  });
});

test("a module that is still starting is reported as try again, not as a failure", async () => {
  const manager = fakeDaemonManager({
    handlers: {
      "orch.task.list": () => {
        throw Object.assign(new Error("orch is starting"), { code: 1100 });
      },
    },
  });
  await assert.rejects(serviceFor(manager).call("task.list"), {
    code: "ORCH_STARTING",
    message: /still starting/,
  });
});

test("a connecting host is waited for; one that never settles times out", async () => {
  const manager = fakeDaemonManager({
    hosts: { local: { state: "connecting" } },
  });
  const service = serviceFor(manager, { readyTimeoutMs: 40 });
  const pending = service.call("task.list");
  setTimeout(() => manager.setState("local", { state: "ready" }), 10);
  await pending;
  assert.equal(manager.calls.length, 1);
  // The listener of the wait is gone again.
  assert.equal(manager.listeners("state"), 0);

  manager.setState("local", { state: "connecting" });
  await assert.rejects(service.call("task.list"), /did not become ready/);
  assert.equal(manager.listeners("state"), 0);
});

test("a remote host that is down is reconnected by the first request, at most every 30 s", async () => {
  let retries = 0;
  const manager = twoHosts({}, (host, m) => {
    retries += 1;
    if (retries === 2) m.setState(host, { state: "ready" });
  });
  manager.setState("devbox-id", {
    state: "failed",
    message: "The connection to the host was lost.",
  });
  const service = serviceFor(manager, { host: REMOTE });
  await assert.rejects(service.call("task.list"), {
    message: "The connection to the host was lost.",
  });
  // A second request right after does not retry again.
  await assert.rejects(service.call("task.list"), {
    message: "The connection to the host was lost.",
  });
  assert.equal(retries, 1);
  // The owner's Try again clears the pause.
  service.forget();
  await service.call("task.list");
  assert.equal(retries, 2);
  assert.equal(manager.requestsFor("orch.task.list").length, 1);
});

test("probe() reports readiness and never connects or retries a host", async () => {
  let retries = 0;
  const manager = twoHosts({}, () => (retries += 1));
  assert.deepEqual(serviceFor(manager).probe(), { pid: 0 });
  manager.setState("devbox-id", { state: "failed", message: "down" });
  assert.throws(() => serviceFor(manager, { host: REMOTE }).probe(), {
    message: "down",
  });
  assert.equal(retries, 0);

  const hosts = new OrchestratorHosts({
    local: serviceFor(manager),
    connections: () => connections,
    getManager: () => manager,
    onChange: () => {},
    createService: () => assert.fail("a probe must not create a service"),
  });
  await assert.rejects(hosts.probe("ssh:other"), /not connected/);
  assert.equal(hosts.services.size, 0);
});

test("task.create is enriched with the repo's enabled MCP launch config", async () => {
  const manager = fakeDaemonManager({
    handlers: { "orch.task.create": (params) => ({ id: "t1", ...params }) },
  });
  const mcpSnapshot = { mcpServers: { fs: { command: "fs-server" } } };
  const service = serviceFor(manager, {
    getClaudeMcp: () => ({ launchConfig: async () => mcpSnapshot }),
  });
  const result = await service.call("task.create", {
    repo: "/Users/sushi/project",
    title: "Export CSV",
  });
  assert.equal(result.id, "t1");
  const sent = manager.requestsFor("orch.task.create")[0];
  assert.deepEqual(sent.params.mcp, mcpSnapshot);
  assert.equal(sent.params.title, "Export CSV");
});

test("task.create proceeds without mcp when the project has no readable MCP config", async () => {
  const manager = fakeDaemonManager({
    handlers: { "orch.task.create": (params) => ({ id: "t2", ...params }) },
  });
  const service = serviceFor(manager, {
    getClaudeMcp: () => ({
      launchConfig: async () => {
        throw new Error("no such project");
      },
    }),
  });
  await service.call("task.create", { repo: "/nope", title: "x" });
  const sent = manager.requestsFor("orch.task.create")[0].params;
  assert.equal("mcp" in sent, false);
});

test("settings.set pushes a full-replace secrets.set: each route's resolved profile env", async () => {
  const settings = {
    routes: [
      {
        id: "r1",
        label: "Sonnet",
        harness: "claude",
        profileId: "prof-1",
        accountId: "account-1",
      },
      { id: "r2", label: "Codex", harness: "codex" },
    ],
  };
  const manager = fakeDaemonManager({
    handlers: {
      "orch.settings.set": () => settings,
      "orch.settings.get": () => settings,
    },
  });
  const service = serviceFor(manager, {
    getModelProviders: () => ({
      resolveEnv: async (id) => {
        if (id !== "prof-1") throw new Error("Model profile not found.");
        return { settings: { ANTHROPIC_MODEL: "x" }, key: "prof-key" };
      },
      resolveClaudeAccount: async (id) => {
        if (id !== "account-1") throw new Error("Claude account not found.");
        return { kind: "subscription", value: "account-token" };
      },
    }),
  });
  await service.call("settings.set", { settings });
  const secretsCall = manager.requestsFor("orch.secrets.set")[0];
  assert.ok(secretsCall, "secrets.set was pushed");
  assert.deepEqual(secretsCall.params, {
    profiles: { "prof-1": { env: { ANTHROPIC_MODEL: "x" }, key: "prof-key" } },
    accounts: {
      "account-1": { kind: "subscription", value: "account-token" },
    },
    projects: {},
    projectMcp: {},
    projectRepos: {},
  });
});

test("secrets.set is always pushed, even to clear it: no profiles means profiles:{}", async () => {
  const settings = { routes: [] };
  const manager = fakeDaemonManager({
    handlers: {
      "orch.settings.set": () => settings,
      "orch.settings.get": () => settings,
    },
  });
  const service = serviceFor(manager, {
    getModelProviders: () => ({
      resolveEnv: async () => {
        throw new Error("should not be called");
      },
    }),
  });
  await service.call("settings.set", { settings });
  const secretsCall = manager.requestsFor("orch.secrets.set")[0];
  assert.deepEqual(secretsCall.params, {
    profiles: {},
    accounts: {},
    projects: {},
    projectMcp: {},
    projectRepos: {},
  });
});

test("a remote daemon receives only trusted project values and project MCP config, never provider keys", async () => {
  const manager = twoHosts({
    "orch.settings.get": () => ({
      routes: [{ id: "r", profileId: "prof-1" }],
    }),
    "orch.task.create": (params) => ({ id: "t1", status: "queued", ...params }),
  });
  const project = {
    id: "project-1",
    mcp: { mcpServers: { lookup: { command: "lookup" } } },
  };
  const service = serviceFor(manager, {
    host: REMOTE,
    getModelProviders: () => ({
      resolveEnv: async () => assert.fail("provider keys stay on this Mac"),
    }),
    getProjects: () => ({
      get: async (id) => (id === project.id ? project : null),
      agentEnvironments: async (target) =>
        target === REMOTE ? { [project.id]: { PROJECT_KEY: "invented" } } : {},
      mcpEnvironments: async (target) =>
        target === REMOTE
          ? { [project.id]: { LOOKUP_TOKEN: "invented-mcp" } }
          : {},
    }),
  });
  const result = await service.call("task.create", {
    repo: "/home/user/sushiai/demo",
    projectId: project.id,
    title: "remote task",
  });
  assert.equal(result.projectId, project.id);
  assert.equal(result.host, REMOTE);
  assert.deepEqual(result.mcp.mcpServers, project.mcp.mcpServers);
  const secrets = manager.requestsFor("orch.secrets.set")[0];
  assert.equal(secrets.host, "devbox-id");
  assert.deepEqual(secrets.params, {
    profiles: {},
    accounts: {},
    projects: { [project.id]: { PROJECT_KEY: "invented" } },
    projectMcp: { [project.id]: { LOOKUP_TOKEN: "invented-mcp" } },
    projectRepos: {},
  });
});

/** Hosts over a fake manager, listening, with `notices` and `relayed`. */
async function listening(manager, extra = {}) {
  const relayed = [];
  const notices = [];
  const hosts = createOrchestratorHosts({
    send: (channel, value) => relayed.push([channel, value]),
    notify: (notice) => notices.push(notice),
    getConnections: () => connections,
    getManager: () => manager,
    enabled: false,
    ...extra,
  });
  await hosts.setEnabled(true);
  return { hosts, relayed, notices };
}

test("orch.event arrives per host: a local one as is, a remote one tagged with its host", async () => {
  const manager = twoHosts({ "orch.task.list": () => [] });
  const { hosts, relayed } = await listening(manager);
  await hosts.call("task.list", {}, REMOTE);
  const task = (id) => ({ id, title: id, repo: "/r", status: "running" });
  manager.notify("local", "orch.event", { event: "task", task: task("l1") });
  manager.notify("devbox-id", "orch.event", {
    event: "task",
    task: task("r1"),
  });
  // A host the app never used, and other notifications, are not relayed.
  manager.notify("other", "orch.event", { event: "task", task: task("x") });
  manager.notify("local", "session.exited", { id: "s" });
  manager.notify("local", "orch.event", { nothing: true });
  assert.deepEqual(relayed, [
    ["orchestrator-event", { event: "task", task: task("l1") }],
    [
      "orchestrator-event",
      {
        event: "task",
        task: { ...task("r1"), host: REMOTE },
        host: REMOTE,
      },
    ],
  ]);
  hosts.close();
  manager.notify("local", "orch.event", { event: "task", task: task("l2") });
  assert.equal(relayed.length, 2);
});

test("events and notices are kept per host: the same task id on two hosts notifies twice", async () => {
  const manager = twoHosts({ "orch.task.list": () => [] });
  const { hosts, notices } = await listening(manager);
  await hosts.call("task.list", {}, REMOTE);
  const waiting = {
    id: "t1",
    title: "Do the thing",
    repo: "/repo",
    status: "waiting",
    question: { text: "Delete old keys?", options: [] },
  };
  manager.notify("local", "orch.event", { event: "task", task: waiting });
  manager.notify("local", "orch.event", { event: "task", task: waiting });
  manager.notify("devbox-id", "orch.event", { event: "task", task: waiting });
  assert.deepEqual(
    notices.map((notice) => [notice.taskId, notice.host]),
    [
      ["t1", undefined],
      ["t1", REMOTE],
    ],
  );
  assert.deepEqual(notices[0], {
    taskId: "t1",
    repo: "/repo",
    kind: "input",
    title: "Do the thing",
    body: "Delete old keys?",
    focus: "question",
    repoName: "repo",
  });
});

test("secrets are pushed when a host becomes ready and again after its daemon restarts", async () => {
  const manager = twoHosts({
    "orch.settings.get": () => ({ routes: [] }),
  });
  const pushed = () => manager.requestsFor("orch.secrets.set");
  const hosts = createOrchestratorHosts({
    send: () => {},
    getConnections: () => connections,
    getManager: () => manager,
    getProjects: () => ({
      agentEnvironments: async () => ({}),
      mcpEnvironments: async () => ({}),
    }),
    enabled: false,
  });
  await hosts.setEnabled(true);
  // Enabling pushes to the local daemon that is ready already.
  assert.equal(pushed().length, 1);
  assert.equal(pushed()[0].host, "local");

  // A daemon that was killed and came back is ready again: pushed again.
  manager.setState("local", { state: "offline" });
  assert.equal(pushed().length, 1);
  manager.setState("local", { state: "ready" });
  await waitUntil(() => pushed().length === 2);

  // A remote host is pushed to once it is in use and ready, not before.
  manager.setState("devbox-id", { state: "offline" });
  manager.setState("devbox-id", { state: "ready" });
  assert.equal(pushed().length, 2);
  await hosts.call("task.list", {}, REMOTE);
  manager.setState("devbox-id", { state: "offline" });
  manager.setState("devbox-id", { state: "ready" });
  await waitUntil(() => pushed().length === 3);
  assert.equal(pushed()[2].host, "devbox-id");
});

test("a lagging subscriber's resync refetches the task list and replays it as events", async () => {
  const manager = fakeDaemonManager({
    handlers: {
      "orch.task.list": () => [
        { id: "a", status: "running", title: "A", repo: "/r" },
        { id: "b", status: "done", title: "B", repo: "/r" },
      ],
    },
  });
  const { relayed } = await listening(manager);
  manager.notify("local", "session.resync", {});
  await waitUntil(() => relayed.length === 2);
  assert.deepEqual(
    relayed.map(([channel, event]) => [channel, event.event, event.task.id]),
    [
      ["orchestrator-event", "task", "a"],
      ["orchestrator-event", "task", "b"],
    ],
  );
});

test("the host list is the daemon manager's states, and a host change is announced", async () => {
  const manager = twoHosts();
  const changes = [];
  const { hosts } = await listening(manager, {
    hostsChanged: () => changes.push(1),
  });
  const byId = () =>
    Object.fromEntries(hosts.list().map((host) => [host.id, host]));
  assert.equal(byId().local.state, "ready");
  assert.equal(byId()[REMOTE].state, "ready");
  assert.equal(byId()[REMOTE].enabled, false);
  manager.setState("devbox-id", { state: "connecting" });
  assert.equal(byId()[REMOTE].state, "connecting");
  manager.setState("devbox-id", { state: "need_auth", message: "Needs a key" });
  assert.equal(byId()[REMOTE].state, "error");
  assert.equal(byId()[REMOTE].detail, "Needs a key");
  manager.setState("devbox-id", { state: "offline", message: "" });
  assert.equal(byId()[REMOTE].state, "idle");
  assert.ok(changes.length >= 3);
});

test("Try again reconnects the host and reads what it offers; Update sushiai runs the host installer", async () => {
  const retried = [];
  const manager = twoHosts({}, (host) => retried.push(host));
  const installed = [];
  const { hosts } = await listening(manager, {
    installHost: async (host) => installed.push(host),
  });
  const preflight = await hosts.preflight(REMOTE);
  assert.deepEqual(retried, ["devbox-id"]);
  assert.equal(preflight.git, true);
  assert.deepEqual(preflight.claude, { installed: true, loggedIn: true });
  assert.equal(
    hosts.list().find((host) => host.id === REMOTE).preflight.git,
    true,
  );
  assert.equal(await hosts.preflight("local"), null);

  await hosts.setup(REMOTE);
  assert.deepEqual(installed, ["devbox-id"]);
  await assert.rejects(hosts.setup("local"), /Invalid orchestrator host/);
});

test("turning the orchestrator off stops listening and sends nothing; on starts again; no daemon is touched", async () => {
  const manager = twoHosts({
    "orch.settings.get": () => ({ routes: [] }),
    "orch.task.list": () => [],
  });
  const { hosts, relayed } = await listening(manager, {
    getProjects: () => ({
      agentEnvironments: async () => ({}),
      mcpEnvironments: async () => ({}),
    }),
  });
  await hosts.call("task.list", {}, REMOTE);
  assert.equal(manager.listeners("event"), 1);
  const before = manager.calls.length;
  await hosts.setEnabled(false);
  assert.equal(manager.listeners("event"), 0);
  assert.equal(manager.listeners("state"), 0);
  for (const call of [
    () => hosts.call("task.list", {}),
    () => hosts.list(),
    () => hosts.probe("local"),
  ])
    await assert.rejects(async () => call(), {
      message: "The orchestrator is off.",
    });
  manager.notify("local", "orch.event", { event: "task", task: { id: "x" } });
  assert.equal(relayed.length, 0);
  assert.equal(manager.calls.length, before);
  assert.equal(
    manager.calls.some((call) => /shutdown/.test(call.method)),
    false,
  );
  await hosts.setEnabled(true);
  assert.equal(manager.listeners("event"), 1);
  assert.equal(hosts.services.size, 0);
});

test("a remote notice names its host; a local one does not", () => {
  const task = { id: "t", repo: "/r", title: "x", status: "waiting" };
  assert.equal("host" in orchestratorNotice(task), false);
  assert.equal(orchestratorNotice({ ...task, host: "local" }).host, undefined);
  assert.equal(
    orchestratorNotice({ ...task, host: "ssh:abc" }).host,
    "ssh:abc",
  );
});

test("the transition detector notifies done and failed once, only for live top-level tasks; a fresh report titles the done notice", () => {
  const notices = [];
  const service = serviceFor(fakeDaemonManager(), {
    notify: (notice) => notices.push(notice),
  });
  const push = (task) => service.handleEvent({ event: "task", task });
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
  push(done);
  push({ ...done, costUsd: 2 });
  push({ ...base, id: "d2", status: "done" });
  push({ ...base, id: "d3", status: "done", report: "# Old", reportAt: 1 });
  push({ ...base, id: "f1", status: "running" });
  push({
    ...base,
    id: "f1",
    status: "failed",
    attempts: [{ n: 1, failure: { kind: "verify" } }],
  });
  // Subtask and archived tasks raise nothing.
  push({ ...base, id: "s1", status: "done", parent: "d1" });
  push({ ...base, id: "s2", status: "failed", parent: "d1" });
  push({ ...base, id: "a1", status: "done", archived: true });
  // A waiting subtask still asks.
  push({
    ...base,
    id: "s3",
    status: "waiting",
    parent: "d1",
    question: { text: "Which?" },
  });
  push({
    ...base,
    id: "a2",
    status: "waiting",
    archived: true,
    question: { text: "Archived?" },
  });
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

test("the transition detector notifies once when a top-level task lands dirty, fails or is stopped by the engine, never for an owner stop", () => {
  const notices = [];
  const service = serviceFor(fakeDaemonManager(), {
    notify: (notice) => notices.push(notice),
  });
  const push = (task) => service.handleEvent({ event: "task", task });
  const base = {
    title: "Ship it",
    repo: "/repo",
    costUsd: 0,
    archived: false,
    attempts: [],
    decisions: [],
  };
  for (const id of ["l1", "e1", "o1", "o2", "f1", "a1"])
    push({ ...base, id, status: "running" });
  const landing = { ...base, id: "l1", status: "landing" };
  push(landing);
  push({ ...landing, costUsd: 1 });
  const engineStopped = {
    ...base,
    id: "e1",
    status: "stopped",
    decisions: ["Orchestrator: no subtasks are left"],
  };
  push(engineStopped);
  push({ ...engineStopped, costUsd: 2 });
  const failed = { ...base, id: "f1", status: "failed" };
  push(failed);
  push({ ...failed, costUsd: 3 });
  push({ ...base, id: "o1", status: "stopped", decisions: ["Owner: stop"] });
  push({
    ...base,
    id: "o2",
    status: "stopped",
    attempts: [{ n: 1, status: "interrupted" }],
  });
  push({ ...base, id: "a1", status: "landing", archived: true });
  // Already landing when first seen: not a transition, no notice.
  push({ ...base, id: "n1", status: "landing" });
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

test("a real project store delivers values and MCP to a remote daemon only once the host is trusted", async (t) => {
  const { makeStore } = require("./helpers/fake-host.cjs");
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "Demo",
    targets: [REMOTE],
    mcp: { mcpServers: { lookup: { command: "lookup" } } },
    env: [
      { name: "PROJECT_KEY", secret: true, availableTo: ["setup", "agent"] },
      { name: "LOOKUP_TOKEN", secret: true, availableTo: ["mcp"] },
    ],
  });
  await projects.setSecret(project.id, "PROJECT_KEY", "invented-key");
  await projects.setSecret(project.id, "LOOKUP_TOKEN", "invented-mcp");
  const pushed = async (trusted) => {
    await projects.setHostWithheld(project.id, REMOTE, !trusted);
    const manager = twoHosts({
      "orch.settings.get": () => ({ routes: [] }),
      "orch.task.create": (params) => params,
    });
    const service = serviceFor(manager, {
      host: REMOTE,
      getProjects: () => projects,
    });
    const task = await service.call("task.create", {
      repo: "/home/user/sushiai/demo",
      projectId: project.id,
      title: "remote task",
    });
    return { task, secrets: manager.requestsFor("orch.secrets.set")[0].params };
  };
  let seen = await pushed(false);
  assert.deepEqual(seen.secrets.projects[project.id] ?? {}, {});
  assert.deepEqual(seen.secrets.projectMcp[project.id] ?? {}, {});
  assert.deepEqual(Object.keys(seen.task.mcp.mcpServers), ["lookup"]);
  seen = await pushed(true);
  assert.deepEqual(seen.secrets.projects[project.id], {
    PROJECT_KEY: "invented-key",
  });
  assert.deepEqual(seen.secrets.projectMcp[project.id], {
    LOOKUP_TOKEN: "invented-mcp",
  });
});

test("a server the project switched off is not handed to a task", async () => {
  const manager = fakeDaemonManager({
    handlers: { "orch.task.create": (params) => params },
  });
  const project = {
    id: "p1",
    mcp: {
      mcpServers: { on: { command: "a" }, off: { command: "b" } },
      disabledMcpServers: ["off"],
    },
  };
  const service = serviceFor(manager, {
    getProjects: () => ({
      get: async () => project,
      resolveDirectory: async () => project,
      agentEnvironments: async () => ({}),
      mcpEnvironments: async () => ({}),
    }),
  });
  const result = await service.call("task.create", {
    repo: "/work/demo",
    projectId: "p1",
    title: "t",
  });
  assert.deepEqual(Object.keys(result.mcp.mcpServers), ["on"]);
});

/** A real store, the real hosts wiring and a service over a fake manager:
 * what the daemon was last told about project values on a host. */
async function sendRig(t) {
  const { makeStore } = require("./helpers/fake-host.cjs");
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "Mine",
    git: { url: "git@example.test:acme/mine.git" },
    targets: [REMOTE],
    env: [{ name: "KEY", secret: true, availableTo: ["setup", "agent"] }],
  });
  await projects.setSecret(project.id, "KEY", "invented-mine");
  const manager = twoHosts({
    "orch.settings.get": () => ({ routes: [] }),
    "orch.task.create": (params) => ({
      id: "t1",
      status: "queued",
      ...params,
    }),
  });
  const service = serviceFor(manager, {
    host: REMOTE,
    getProjects: () => projects,
  });
  // The real wiring: the hosts wrapper sets the store's change hooks.
  const hosts = createOrchestratorHosts({
    send: () => {},
    getProjects: () => projects,
    getConnections: () => connections,
    getManager: () => manager,
    enabled: false,
  });
  hosts.services.set(REMOTE, service);
  const pushes = () => manager.requestsFor("orch.secrets.set");
  const lastSecrets = () => pushes().at(-1)?.params;
  return { projects, project, manager, service, pushes, lastSecrets };
}

test("a host the owner added gets a task's project values with no approval step", async (t) => {
  const { project, service, lastSecrets } = await sendRig(t);
  await service.call("task.create", {
    repo: "/home/user/sushiai/mine",
    projectId: project.id,
    title: "remote task",
  });
  assert.deepEqual(lastSecrets().projects[project.id], {
    KEY: "invented-mine",
  });
});

test("switching sending off empties the daemon's copy at once, and switching it on gives the values back", async (t) => {
  const { projects, project, service, pushes, lastSecrets } = await sendRig(t);
  await service.call("task.list", {}).catch(() => {});
  await service.refreshSecrets();
  assert.equal(lastSecrets().projects[project.id].KEY, "invented-mine");
  const before = pushes().length;
  await projects.setHostWithheld(project.id, REMOTE, true);
  await waitUntil(() => pushes().length > before);
  await waitUntil(() => !lastSecrets().projects[project.id]?.KEY);
  assert.deepEqual(lastSecrets().projects[project.id] ?? {}, {});
  // A task made while it is off carries no value either.
  await service.call("task.create", {
    repo: "/home/user/sushiai/mine",
    projectId: project.id,
    title: "later",
  });
  assert.deepEqual(lastSecrets().projects[project.id] ?? {}, {});
  await projects.setHostWithheld(project.id, REMOTE, false);
  await waitUntil(
    () => lastSecrets().projects[project.id]?.KEY === "invented-mine",
  );
});

test("a task started in a folder without naming its project still gets that project's values, and an edit reaches the host at once", async (t) => {
  const { projects, project, service, pushes, lastSecrets } = await sendRig(t);
  await projects.attach({
    remote: "git@example.test:acme/mine.git",
    endpoint: REMOTE,
    cwd: "/home/user/sushiai/mine",
    name: "Mine",
  });
  // The agent's own task.create names no project: the folder says which.
  const made = await service.call("task.create", {
    repo: "/home/user/sushiai/mine",
    title: "from chat",
  });
  assert.equal(made.projectId, project.id);
  assert.equal(
    lastSecrets().projectRepos["/home/user/sushiai/mine"],
    project.id,
  );
  // An edit in Project settings is pushed without waiting for a task.
  const before = pushes().length;
  await projects.setSecret(project.id, "KEY", "changed-value");
  await waitUntil(
    () =>
      pushes().length > before &&
      lastSecrets().projects[project.id]?.KEY === "changed-value",
    { timeout: 4000 },
  );
});

test("a host gets only the projects that run on it", async (t) => {
  const { projects, project } = await sendRig(t);
  const stranger = await projects.upsert({
    name: "Stranger",
    env: [{ name: "OTHER", secret: true }],
  });
  await projects.setSecret(stranger.id, "OTHER", "not-for-this-host");
  const sent = await projects.agentEnvironments(REMOTE);
  assert.deepEqual(Object.keys(sent), [project.id]);
  assert.deepEqual(
    Object.keys(await projects.agentEnvironments("local")).sort(),
    [project.id, stranger.id].sort(),
  );
});
