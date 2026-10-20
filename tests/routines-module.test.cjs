// The Routines module's desktop side with a fake daemon manager: the catalog
// the daemon validates against, the builder session, the events and the
// on/off switch. Synthetic hosts and projects only.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { until } = require("./helpers/deadline.cjs");
const {
  createRoutines,
  builderArgs,
  builderPrompt,
  builderSettings,
  loginTerminalCommand,
} = require("../electron/routines.cjs");
const {
  assembleCatalog,
  daemonCatalog,
  standardAgent,
} = require("../electron/routines-catalog.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

const MODELS = {
  claude: {
    models: [
      { id: "sonnet", label: "Sonnet" },
      { id: "opus", label: "Opus" },
    ],
  },
  codex: { models: [{ id: "gpt-local", label: "GPT Local" }] },
};
const PROJECTS = [
  {
    id: "p1",
    name: "my-app",
    purpose: "The storefront",
    git: { url: "git@example.com:org/my-app.git" },
    folders: [
      { endpoint: "local", cwd: "/work/my-app" },
      { endpoint: "ssh:build-host", cwd: "/srv/my-app" },
    ],
  },
];

function world({
  enabled = true,
  caps = ["routines"],
  settings = null,
  cwd = () => "/tmp",
  userDataDir,
  execs = [],
  reads = {},
} = {}) {
  const manager = fakeDaemonManager({
    hosts: {
      local: { capabilities: caps },
      "build-host": { capabilities: caps },
    },
    handlers: {
      "session.read": (p) => {
        if (!(p.id in reads)) throw new Error("session is not running");
        return { text: reads[p.id] };
      },
      "routines.list": (_p, host) => ({
        routines: [{ routine: { id: `r-${host}` } }],
      }),
    },
  });
  manager.hello = (host) => ({ host: host === "local" ? "my-mac" : "bh" });
  const sent = [];
  const launched = [];
  const switched = [];
  const stored = {};
  const flag = { enabled };
  const routines = createRoutines({
    getManager: () => manager,
    getConnections: () => ({
      list: () => [{ id: "build-host", name: "build-host" }],
      get: () => ({}),
      exec: async (endpoint, command) => {
        execs.push({ endpoint, command });
        return `/home/u/${/\$HOME\/([^"]+)"/.exec(command)?.[1] ?? ".sushiai/routines/_builder"}\n`;
      },
    }),
    userDataDir,
    getProjects: () => ({ list: async () => PROJECTS }),
    isEnabled: () => flag.enabled,
    moduleSwitch: {
      enable: async (host) => switched.push(`enable ${host}`),
      disable: async (host) => switched.push(`disable ${host}`),
    },
    send: (channel, value) => sent.push({ channel, value }),
    readStore: (name) => stored[name],
    writeStore: (name, value) => {
      stored[name] = value;
    },
    chatModels: () => MODELS,
    probeHost: async (host) => ({
      claude: { installed: true, loggedIn: true },
      codex: { installed: true, loggedIn: host === "local" },
      codexModels: [{ id: "gpt-remote", label: "GPT Remote" }],
    }),
    standardSettings: async () => settings,
    launch: async (request) => {
      launched.push(request);
      return { sessionId: `s${launched.length}` };
    },
    builderCwd: cwd,
    turnPollMs: 5,
    log: () => {},
  });
  return { routines, manager, sent, launched, switched, stored, flag };
}

test("the catalog carries each host's harness login, its own models and the project purpose", async () => {
  const { routines } = world();
  const catalog = await routines.catalog();
  const mac = catalog.hosts.find((h) => h.id === "local");
  const box = catalog.hosts.find((h) => h.id === "ssh:build-host");
  assert.equal(mac.hostName, "my-mac");
  assert.deepEqual(
    mac.harnesses.codex.models.map((m) => m.id),
    ["gpt-local"],
  );
  assert.deepEqual(
    box.harnesses.codex.models.map((m) => m.id),
    ["gpt-remote"],
  );
  assert.equal(mac.harnesses.codex.loggedIn, true);
  assert.equal(box.harnesses.codex.loggedIn, false);
  const project = catalog.projects[0];
  assert.equal(project.purpose, "The storefront");
  assert.deepEqual(
    project.folders.map((f) => f.host),
    ["local", "ssh:build-host"],
  );
});

test("the default agent is the Standard route; without one it is Claude Sonnet", () => {
  const settings = {
    tiers: { standard: "r2" },
    routes: [
      { id: "r1", harness: "claude", model: "opus", effort: "high" },
      { id: "r2", harness: "codex", model: "gpt-local", effort: "low" },
    ],
  };
  assert.deepEqual(standardAgent(settings), {
    harness: "codex",
    model: "gpt-local",
    effort: "low",
  });
  assert.deepEqual(standardAgent(null), {
    harness: "claude",
    model: "sonnet",
    effort: "medium",
  });
  const catalog = assembleCatalog({
    hosts: [],
    models: MODELS,
    projects: [],
    settings,
  });
  assert.equal(catalog.defaults.agent.harness, "codex");
});

test("the daemon catalog names only its own host and the folders on it", () => {
  const catalog = assembleCatalog({
    hosts: [
      { id: "local", name: "This Mac", state: "ready", hostName: "my-mac" },
      {
        id: "ssh:build-host",
        name: "build-host",
        state: "ready",
        hostName: "bh",
      },
    ],
    facts: {
      "ssh:build-host": {
        claude: { installed: true, loggedIn: false },
        codex: { installed: false, loggedIn: false },
        codexModels: [],
      },
    },
    models: MODELS,
    projects: PROJECTS,
  });
  const wire = daemonCatalog(catalog, "ssh:build-host");
  assert.deepEqual(
    wire.hosts.map((h) => h.id),
    ["bh"],
  );
  assert.equal(wire.hosts[0].harnesses.claude.loggedIn, false);
  assert.deepEqual(wire.projects[0].folders, [
    { host: "bh", path: "/srv/my-app" },
  ]);
});

test("a save reaches the host's daemon with the catalog and the routine named for that host", async () => {
  const { routines, manager } = world();
  manager.handlers["routines.save"] = () => ({ id: "digest", enabled: false });
  await routines.methods.save("ssh:build-host", {
    routine: {
      id: "digest",
      name: "Digest",
      where: { host: "local", projects: ["p1"] },
      agent: {
        harness: "codex",
        model: "gpt-remote",
        effort: "low",
        extraArgs: [],
      },
    },
    brief: "Summarise.",
  });
  const [call] = manager.requestsFor("routines.save");
  assert.equal(call.host, "build-host");
  assert.equal(call.params.routine.where.host, "bh");
  assert.equal(call.params.routine.agent.model, "gpt-remote");
  assert.equal(call.params.routine.agent.effort, "low");
  assert.equal(call.params.brief, "Summarise.");
  assert.deepEqual(call.params.catalog.hosts[0].harnesses.codex.models, [
    "gpt-remote",
  ]);
});

const APPROVALS = [
  "routines_projects",
  "routines_hosts",
  "routines_skills",
  "routines_list",
  "routines_show",
  "routines_validate",
  "routines_save",
  "routines_dry_run",
].flatMap((tool) => [
  "-c",
  `mcp_servers.sushiai.tools.${tool}.approval_mode="approve"`,
]);

test("the builder session carries the owner's text, the Settings route and runs in the background", async () => {
  const { routines, launched } = world();
  routines.methods["settings.set"]({
    harness: "codex",
    model: "gpt-local",
    effort: "high",
    host: "local",
  });
  const started = await routines.methods["builder.start"]({
    text: "Every weekday at 9 summarise new issues of my-app",
  });
  assert.equal(started.sessionId, "s1");
  const [request] = launched;
  assert.equal(request.agent, "codex");
  assert.equal(request.model, "gpt-local");
  assert.equal(request.background, true);
  assert.match(
    request.prompt,
    /Every weekday at 9 summarise new issues of my-app/,
  );
  assert.match(request.prompt, /sushiai-routines/);
  assert.equal(request.headless, true);
  assert.deepEqual(request.extraArgs, [
    "--ignore-user-config",
    "-c",
    'sandbox_mode="workspace-write"',
    "-c",
    "sandbox_workspace_write.network_access=false",
    "-c",
    "sandbox_workspace_write.exclude_slash_tmp=true",
    "-c",
    "sandbox_workspace_write.exclude_tmpdir_env_var=true",
    "-c",
    "model_reasoning_effort=high",
    "-c",
    "mcp_servers={}",
    "-c",
    'mcp_servers.sushiai.command="sushiai"',
    "-c",
    'mcp_servers.sushiai.args=["mcp","--routines"]',
    ...APPROVALS,
  ]);
});

const ANSWER = (result, session) =>
  JSON.stringify({
    type: "result",
    result,
    session_id: session,
    is_error: false,
  });

test("a Claude builder turn runs headless and resumes the previous conversation", async () => {
  const reads = { s1: "" };
  const { routines, launched } = world({ reads });
  const started = await routines.methods["builder.start"]({ text: "digest" });
  const [first] = launched;
  assert.equal(started.harness, "claude");
  assert.equal(started.turn.state, "running");
  assert.equal(first.headless, true);
  assert.deepEqual(first.extraArgs.slice(0, 2), ["--output-format", "json"]);
  assert.ok(!first.extraArgs.includes("-p"));
  assert.equal(first.resume, undefined);
  assert.equal(first.background, true);
  const tools = first.extraArgs[first.extraArgs.indexOf("--allowedTools") + 1];
  assert.ok(tools.split(" ").includes("Edit(./**)"));
  assert.doesNotMatch(tools, /\bWrite\b|sushiai ask/);
  assert.match(first.prompt, /non-interactive turn/);
  assert.match(first.prompt, /sushiai is not available in this session/);

  await assert.rejects(
    routines.methods["builder.reply"]({ sessionId: "s1", text: "more" }),
    /still working/,
  );
  // The answer is wrapped by the terminal and the session exits right after.
  const json = ANSWER("Done: **draft** saved.", "agent-1");
  reads.s1 = `${json.slice(0, 40)}\r\n${json.slice(40)}`;
  const turn = await until(() => {
    const view = routines.methods["builder.turn"]({ sessionId: "s1" });
    return view.state !== "running" && view;
  }, "the builder turn to end");
  delete reads.s1;
  assert.deepEqual(turn, {
    state: "done",
    text: "Done: **draft** saved.",
    agentSession: "agent-1",
  });

  const next = await routines.methods["builder.reply"]({
    sessionId: "s1",
    text: "only tasks tagged bot",
  });
  const second = launched[1];
  assert.equal(next.sessionId, "s2");
  assert.equal(next.agentSession, "agent-1");
  assert.equal(second.resume, "agent-1");
  assert.equal(second.prompt, "only tasks tagged bot");
  assert.equal(second.cwd, first.cwd);
  assert.deepEqual(second.extraArgs, first.extraArgs);
  assert.notEqual(second.idempotencyKey, first.idempotencyKey);
});

test("a session that prints the answer and then refuses reads still settles done", async () => {
  let served = false;
  const reads = new Proxy(
    {},
    {
      has: (_t, id) => id === "s1" && !served,
      get: (_t, id) => {
        served = true;
        return id === "s1" ? ANSWER("Saved the draft.", "agent-9") : undefined;
      },
    },
  );
  const { routines } = world({ reads });
  await routines.methods["builder.start"]({ text: "digest" });
  const turn = await until(() => {
    const view = routines.methods["builder.turn"]({ sessionId: "s1" });
    return view.state !== "running" && view;
  }, "the builder turn to end");
  assert.deepEqual(turn, {
    state: "done",
    text: "Saved the draft.",
    agentSession: "agent-9",
  });
});

test("the codex builder always uses mcp access and a bare config", async () => {
  assert.equal(
    builderSettings({ harness: "codex", access: "cli" }).access,
    "mcp",
  );
  assert.equal(builderSettings({ harness: "codex" }).access, "mcp");
  assert.equal(
    builderSettings({ harness: "codex", access: "both" }).access,
    "both",
  );
  assert.equal(
    builderSettings({ harness: "claude", access: "cli" }).access,
    "cli",
  );
  const { routines, launched } = world();
  routines.methods["settings.set"]({ harness: "codex", access: "cli" });
  assert.equal(routines.methods["settings.get"]().access, "mcp");
  await routines.methods["builder.start"]({ text: "Every day do X" });
  const args = launched[0].extraArgs;
  assert.ok(args.includes('mcp_servers.sushiai.command="sushiai"'));
  assert.ok(
    args.indexOf("mcp_servers={}") <
      args.indexOf('mcp_servers.sushiai.command="sushiai"'),
  );
});

test("a builder turn that ends without an answer fails with what the session printed", async () => {
  // The module polls the session; it must read the error text once before
  // the session disappears.
  let text = "";
  let seen = false;
  const reads = {
    get s1() {
      if (text) seen = true;
      return text;
    },
  };
  const { routines } = world({ reads });
  await routines.methods["builder.start"]({ text: "digest" });
  text = "Error: not logged in";
  await until(() => seen, "the poll to read the error");
  delete reads.s1;
  const turn = await until(() => {
    const view = routines.methods["builder.turn"]({ sessionId: "s1" });
    return view.state !== "running" && view;
  }, "the builder turn to end");
  assert.equal(turn.state, "failed");
  assert.match(turn.text, /not logged in/);
  await assert.rejects(
    routines.methods["builder.reply"]({ sessionId: "s1", text: "again" }),
    /did not report/,
  );
});

const CODEX_EVENTS = (text, thread) =>
  [
    { type: "thread.started", thread_id: thread },
    { type: "item.completed", item: { type: "error", message: "deprecated" } },
    { type: "turn.started" },
    { type: "item.completed", item: { type: "agent_message", text } },
    { type: "turn.completed", usage: {} },
  ]
    .map((event) => JSON.stringify(event))
    .join("\n");

test("a Codex builder turn runs headless and resumes by its thread id", async () => {
  const reads = { s1: "" };
  const { routines, launched } = world({ reads });
  routines.methods["settings.set"]({ harness: "codex" });
  const started = await routines.methods["builder.start"]({ text: "digest" });
  assert.equal(started.turn.state, "running");
  assert.equal(launched[0].headless, true);
  assert.ok(launched[0].extraArgs.includes('sandbox_mode="workspace-write"'));
  assert.ok(
    launched[0].extraArgs.includes(
      "sandbox_workspace_write.network_access=false",
    ),
  );
  assert.match(launched[0].prompt, /non-interactive turn/);
  // Terminal noise around the events and a wrapped agent message.
  const events = CODEX_EVENTS("A long answer: " + "word ".repeat(30), "t-1");
  const at = events.indexOf("agent_message") + 20;
  reads.s1 = `Reading input\nERROR rmcp: closed\n${events.slice(0, at)}\r\n${events.slice(at)}`;
  const turn = await until(() => {
    const view = routines.methods["builder.turn"]({ sessionId: "s1" });
    return view.state !== "running" && view;
  }, "the builder turn to end");
  delete reads.s1;
  assert.deepEqual(turn, {
    state: "done",
    text: "A long answer: " + "word ".repeat(30),
    agentSession: "t-1",
  });
  await routines.methods["builder.reply"]({ sessionId: "s1", text: "more" });
  assert.equal(launched[1].resume, "t-1");
  assert.deepEqual(launched[1].extraArgs, launched[0].extraArgs);
});

test("a failed Codex turn reports its message", async () => {
  const reads = {
    s1: [
      '{"type":"thread.started","thread_id":"t-2"}',
      '{"type":"turn.failed","error":{"message":"not logged in"}}',
    ].join("\n"),
  };
  const { routines } = world({ reads });
  routines.methods["settings.set"]({ harness: "codex" });
  await routines.methods["builder.start"]({ text: "digest" });
  const turn = await until(() => {
    const view = routines.methods["builder.turn"]({ sessionId: "s1" });
    return view.state !== "running" && view;
  }, "the builder turn to end");
  assert.equal(turn.state, "failed");
  assert.equal(turn.text, "not logged in");
});

test("builder settings repair an effort the harness lacks and a bad host", () => {
  const value = builderSettings({ harness: "codex", effort: "max", host: "x" });
  assert.equal(value.effort, "medium");
  assert.equal(value.host, "local");
  assert.deepEqual(
    builderArgs({ harness: "claude", effort: "max" }).slice(2, 4),
    ["--effort", "max"],
  );
  assert.match(builderPrompt("Do X", "tracker-intake"), /tracker-intake/);
});

test("the builder prompt tells both harnesses to answer in the owner's language", () => {
  for (const harness of ["claude", "codex"]) {
    const prompt = builderPrompt("Do X", null, {
      ...builderSettings({ harness }),
    });
    assert.match(prompt, /Reply in the language of the owner's request/);
  }
});

test("the builder prompt forbids the run result JSON in its own reply", () => {
  for (const harness of ["claude", "codex"]) {
    const prompt = builderPrompt("Do X", null, {
      ...builderSettings({ harness }),
    });
    assert.match(prompt, /plain prose for the owner/);
    assert.match(prompt, /never print the run result JSON/);
  }
});

test("the builder may write only under its own folder", () => {
  const args = builderArgs({ harness: "claude", effort: "max", access: "cli" });
  const tools = args[args.indexOf("--allowedTools") + 1].split(" ");
  assert.ok(tools.includes("Edit(./**)"));
  assert.ok(!tools.includes("Write"));
});

test("routines.event is forwarded tagged with its module and host", () => {
  const { routines, manager, sent } = world();
  routines.listen();
  manager.notify("build-host", "routines.event", {
    kind: "run_finished",
    routineId: "digest",
    inbox: false,
  });
  manager.notify("build-host", "orch.event", { event: "task" });
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0], {
    channel: "module-event",
    value: {
      moduleId: "builtin.routines",
      host: "ssh:build-host",
      event: { kind: "run_finished", routineId: "digest", inbox: false },
    },
  });
});

test("turning Routines off suspends the schedules, then unregisters the module", async () => {
  const { routines, manager, switched } = world();
  await routines.disable();
  assert.deepEqual(
    manager.requestsFor("routines.suspend").map((c) => c.host),
    ["local", "build-host"],
  );
  assert.deepEqual(switched.sort(), [
    "disable local",
    "disable ssh:build-host",
  ]);
});

test("a host that does not serve the module is switched on before its first request", async () => {
  const { routines, manager, switched } = world({ caps: [] });
  manager.handlers["routines.runs"] = () => ({ runs: [] });
  await routines.methods.runs("ssh:build-host", "digest");
  assert.deepEqual(switched, ["enable ssh:build-host"]);
});

test("every request is refused while the extension is off", async () => {
  const { routines } = world({ enabled: false });
  await assert.rejects(routines.methods.list(), /Routines are off/);
  await assert.rejects(
    routines.methods["builder.start"]({ text: "x" }),
    /Routines are off/,
  );
});

test("a host that was offline when Routines went off is retired when it turns ready", async () => {
  const { routines, manager, switched, flag } = world({ enabled: false });
  manager.setState("build-host", { state: "offline" });
  routines.watchHosts();
  await new Promise((r) => setImmediate(r));
  // local was ready at start and is retired; the offline host is untouched.
  assert.deepEqual(
    manager.requestsFor("routines.suspend").map((c) => c.host),
    ["local"],
  );
  manager.setState("local", { capabilities: [] }); // unregistered now
  manager.setState("build-host", {
    state: "ready",
    capabilities: ["routines"],
  });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(
    manager.requestsFor("routines.suspend").map((c) => c.host),
    ["local", "build-host"],
  );
  assert.ok(switched.includes("disable ssh:build-host"));
  assert.equal(flag.enabled, false);
});

test("turning Routines off then on serves the module again on the ssh hosts it served", async () => {
  const { routines, manager, switched, flag } = world({ enabled: true });
  await routines.disable();
  assert.ok(switched.includes("disable ssh:build-host"));
  // The module is unregistered everywhere now.
  manager.setState("local", { capabilities: [] });
  manager.setState("build-host", { capabilities: [] });
  switched.length = 0;
  flag.enabled = true;
  await routines.enable();
  assert.ok(switched.includes("enable local"));
  assert.ok(switched.includes("enable ssh:build-host"), switched.join());
});

test("a host that was offline at enable gets the module when it turns ready", async () => {
  const { routines, manager, switched, flag } = world({ enabled: true });
  await routines.disable();
  manager.setState("local", { capabilities: [] });
  manager.setState("build-host", { state: "offline", capabilities: [] });
  switched.length = 0;
  flag.enabled = true;
  routines.watchHosts();
  await routines.enable();
  assert.deepEqual(switched, ["enable local"]);
  manager.setState("build-host", { state: "ready" });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(switched, ["enable local", "enable ssh:build-host"]);
});

test("a host that turns ready is left alone while Routines is on", async () => {
  const { routines, manager, switched } = world({ enabled: true });
  routines.watchHosts();
  manager.setState("build-host", {
    state: "ready",
    capabilities: ["routines"],
  });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(manager.requestsFor("routines.suspend"), []);
  assert.deepEqual(switched, []);
});

test("the builder works in a private folder: app data locally, ~/.sushiai over ssh", async () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const data = fs.mkdtempSync(path.join(os.tmpdir(), "routines-data-"));
  try {
    const execs = [];
    const w = world({ cwd: null, userDataDir: data, execs });
    await w.routines.methods["builder.start"]({ text: "digest" });
    const cwd = w.launched[0].cwd;
    assert.ok(cwd.startsWith(data), cwd);
    assert.equal(fs.statSync(cwd).mode & 0o077, 0);
    w.stored["routines-settings"] = { host: "ssh:build-host" };
    await w.routines.methods["builder.start"]({ text: "digest" });
    assert.match(
      w.launched[1].cwd,
      /^\/home\/u\/\.sushiai\/routines\/_builder\/[0-9a-f-]{36}$/,
    );
    assert.match(execs[0].command, /\.sushiai\/routines\/_builder\//);
    // Every build gets its own folder, so resume never mixes two builds.
    await w.routines.methods["builder.start"]({ text: "digest" });
    assert.notEqual(w.launched[2].cwd, w.launched[1].cwd);
    assert.notEqual(w.launched[0].cwd, w.launched[2].cwd);
    assert.ok(w.launched[0].cwd.endsWith(path.basename(w.launched[0].cwd)));
    assert.doesNotMatch(execs[0].command, /\/tmp/);
  } finally {
    fs.rmSync(data, { recursive: true, force: true });
  }
});

test("builder settings keep what it sees and how it reaches sushiai, and repair bad values", () => {
  const { routines, stored } = world();
  assert.deepEqual(routines.methods["settings.get"](), {
    harness: "claude",
    model: "",
    effort: "medium",
    host: "local",
    accountId: "",
    sees: { projects: true, connections: true },
    access: "cli",
  });
  // A patch of one `sees` key keeps the other.
  const next = routines.methods["settings.set"]({
    sees: { projects: false },
    access: "both",
  });
  assert.deepEqual(next.sees, { projects: false, connections: true });
  assert.equal(next.access, "both");
  assert.deepEqual(stored["routines-settings"].sees, next.sees);
  const bad = builderSettings({ access: "ssh", sees: { projects: "no" } });
  assert.equal(bad.access, "cli");
  assert.deepEqual(bad.sees, { projects: true, connections: true });
});

test("the builder gets the sushiai MCP server only when the owner chose mcp or both", async () => {
  const { routines, launched } = world();
  routines.methods["settings.set"]({ access: "cli" });
  await routines.methods["builder.start"]({ text: "Every day do X" });
  assert.equal(launched[0].extraArgs.includes("--mcp-config"), false);
  assert.doesNotMatch(launched[0].extraArgs.join(" "), /mcp__/);

  routines.methods["settings.set"]({ access: "mcp" });
  await routines.methods["builder.start"]({ text: "Every day do X" });
  const args = launched[1].extraArgs;
  const config = JSON.parse(args[args.indexOf("--mcp-config") + 1]);
  assert.deepEqual(config.mcpServers.sushiai, {
    command: "sushiai",
    args: ["mcp", "--routines"],
  });
  const tools = args[args.indexOf("--allowedTools") + 1].split(" ");
  assert.ok(tools.includes("mcp__sushiai__routines_save"));
  // The door never offers the in-run commands.
  assert.ok(!tools.some((tool) => /^mcp__.*(ask|propose)/.test(tool)));
  assert.match(launched[1].prompt, /routines_validate/);

  routines.methods["settings.set"]({ access: "both", harness: "codex" });
  await routines.methods["builder.start"]({ text: "Every day do X" });
  const codex = launched[2].extraArgs;
  assert.ok(codex.includes('mcp_servers.sushiai.command="sushiai"'));
  assert.ok(codex.includes('mcp_servers.sushiai.args=["mcp","--routines"]'));
});

test("the builder prompt respects what the owner lets it see", async () => {
  const { routines, launched } = world();
  await routines.methods["builder.start"]({ text: "Every day do X" });
  assert.doesNotMatch(launched[0].prompt, /Owner's limits/);
  routines.methods["settings.set"]({
    sees: { projects: false, connections: false },
  });
  await routines.methods["builder.start"]({ text: "Every day do X" });
  assert.match(launched[1].prompt, /Do not read the owner's project files/);
  assert.match(launched[1].prompt, /Do not propose connector or MCP tools/);
  routines.methods["settings.set"]({ sees: { projects: true } });
  await routines.methods["builder.start"]({ text: "Every day do X" });
  assert.doesNotMatch(launched[2].prompt, /project files/);
  assert.match(launched[2].prompt, /connector or MCP tools/);
});

test("grant, revoke and approve go to the routine's own host", async () => {
  const { routines, manager } = world();
  await routines.methods.grant("ssh:build-host", "r1", ["Read"]);
  await routines.methods.grant("local", "r1", "all");
  await routines.methods.revoke("local", "r1", ["Read"]);
  await routines.methods.approve("ssh:build-host", "r1", 4, 0);
  assert.deepEqual(manager.requestsFor("routines.grant")[0].params, {
    id: "r1",
    tools: ["Read"],
  });
  assert.equal(manager.requestsFor("routines.grant")[0].host, "build-host");
  assert.deepEqual(manager.requestsFor("routines.grant")[1].params, {
    id: "r1",
    all: true,
  });
  assert.deepEqual(manager.requestsFor("routines.revoke")[0].params, {
    id: "r1",
    tools: ["Read"],
  });
  assert.deepEqual(manager.requestsFor("routines.approve")[0].params, {
    id: "r1",
    run: 4,
    proposal: 0,
  });
});

test("the login terminal is a plain shell session that runs only the harness login", async () => {
  const execs = [];
  const { routines, manager } = world({ execs });
  manager.handlers["session.create"] = () => ({ id: "login-1" });
  const opened = await routines.methods["open-login-terminal"]({
    host: "ssh:build-host",
    harness: "claude",
  });
  assert.deepEqual(opened, {
    host: "ssh:build-host",
    sessionHost: "build-host",
    sessionId: "login-1",
  });
  const [call] = manager.requestsFor("session.create");
  assert.equal(call.host, "build-host");
  assert.equal(call.params.agent, undefined);
  assert.equal(call.params.prompt, undefined);
  assert.deepEqual(call.params.cmd, loginTerminalCommand("claude"));
  assert.match(call.params.cmd.join(" "), /claude \/login/);
  assert.match(execs.at(-1).command, /\$HOME/);
  // The name a run recorded resolves to the host too.
  await routines.methods["open-login-terminal"]({
    host: "bh",
    harness: "codex",
  });
  const second = manager.requestsFor("session.create")[1];
  assert.match(second.params.cmd.join(" "), /codex login/);
  assert.equal(second.host, "build-host");
  await assert.rejects(
    routines.methods["open-login-terminal"]({ host: "local", harness: "rm" }),
    /claude or codex/,
  );
  await assert.rejects(
    routines.methods["open-login-terminal"]({
      host: "ssh:nowhere",
      harness: "claude",
    }),
    /Unknown host|not ready/,
  );
});

test("the login command finds a harness that is on the login shell's PATH only", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const { spawnSync } = require("node:child_process");
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "routines-login-"));
  try {
    const bin = path.join(home, ".local", "bin");
    fs.mkdirSync(bin, { recursive: true });
    for (const name of ["claude", "codex"]) {
      fs.writeFileSync(
        path.join(bin, name),
        `#!/bin/sh\necho "fake-${name} $*"\n`,
        { mode: 0o755 },
      );
    }
    fs.writeFileSync(
      path.join(home, ".bash_profile"),
      'export PATH="$HOME/.local/bin:$PATH"\n',
    );
    const fakePath = { PATH: "/usr/bin:/bin", HOME: home };
    // The harness is not on the non-login PATH the daemon would give a session.
    const plain = spawnSync("/bin/sh", ["-c", "command -v claude"], {
      env: fakePath,
    });
    assert.notEqual(plain.status, 0);
    for (const [harness, line] of [
      ["claude", "fake-claude /login"],
      ["codex", "fake-codex login"],
    ]) {
      const argv = loginTerminalCommand(harness);
      const ran = spawnSync(argv[0], argv.slice(1), {
        env: fakePath,
        input: "",
        encoding: "utf8",
      });
      assert.match(ran.stdout, new RegExp(line), ran.stderr);
    }
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});
