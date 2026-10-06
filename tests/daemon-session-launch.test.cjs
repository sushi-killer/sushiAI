const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const {
  createSessionLauncher,
  SHELL_CMD,
} = require("../electron/session-launch.cjs");
const { sessionLaunchEnv } = require("../electron/project-session.cjs");
const { registerDaemonIpc } = require("../electron/ipc/daemon.cjs");
const { toDaemonLaunch } = require("../src/workspace/session-launch.ts");

const TOKEN = "tok-synthetic-subscription-123";
const API_KEY = "key-synthetic-api-456";
const MODEL_KEY = "key-synthetic-model-789";
const CODEX_HOME = "/tmp/codex-account-home";

function tempDir(t, prefix = "l4a-") {
  // Short path under /tmp: unix socket paths are limited on macOS.
  const dir = fs.mkdtempSync(path.join("/tmp", prefix));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function setup(t, { project = null, sends = true, accounts = {} } = {}) {
  const requests = [];
  const manager = {
    request: async (host, method, params) => {
      requests.push({ host, method, params });
      return { id: "s1" };
    },
  };
  const projects = {
    resolveProject: async () => project,
    sendsValues: async () => sends,
    environmentFor: async () => ({ PROJECT_VALUE: "pv" }),
  };
  const launcher = createSessionLauncher({
    manager,
    worktreeCreate: async (root, branch, base) => {
      const target = path.join(path.dirname(root), `wt-${branch}`);
      fs.mkdirSync(target, { recursive: true });
      setup.worktrees.push({ root, branch, base });
      return { path: target };
    },
    environment: (input) =>
      sessionLaunchEnv(
        {
          projects,
          resolveAccount: async (id) => accounts.claude?.[id],
          resolveCodexAccount: async (id) => accounts.codex?.[id],
          resolveModel: async () => ({
            key: MODEL_KEY,
            settings: { ANTHROPIC_BASE_URL: "https://models.invalid" },
          }),
        },
        input,
      ),
  });
  const cwd = fs.realpathSync(tempDir(t));
  return { launcher, requests, cwd };
}
setup.worktrees = [];

const base = (cwd, extra = {}) => ({
  host: "local",
  cwd,
  cols: 100,
  rows: 30,
  idempotencyKey: "key-1",
  ...extra,
});

test("shell launch sends a login shell command with no agent and the workspace as group", async (t) => {
  const { launcher, requests, cwd } = setup(t);
  const out = await launcher.launch(
    base(cwd, { group: "ws-1", title: "Shell" }),
  );
  assert.deepEqual(out, {
    host: "local",
    sessionId: "s1",
    cwd: require("node:fs").realpathSync(cwd),
  });
  const [call] = requests;
  assert.equal(call.host, "local");
  assert.equal(call.method, "session.create");
  assert.deepEqual(call.params.cmd, SHELL_CMD);
  assert.equal(call.params.agent, undefined);
  assert.equal(call.params.group, "ws-1");
  assert.equal(call.params.idempotencyKey, "key-1");
  assert.equal(call.params.cwd, cwd);
});

test("subscription token goes to env and never into claudeSettings", async (t) => {
  const { launcher, requests, cwd } = setup(t, {
    accounts: { claude: { a1: { kind: "subscription", value: TOKEN } } },
  });
  await launcher.launch(
    base(cwd, {
      agent: "claude",
      claudeAccountId: "a1",
      extraArgs: ["--effort", "high"],
    }),
  );
  const { params } = requests[0];
  assert.equal(params.env.CLAUDE_CODE_OAUTH_TOKEN, TOKEN);
  assert.equal(params.agent, "claude");
  assert.equal(params.cmd, undefined);
  assert.deepEqual(params.extraArgs, ["--effort", "high"]);
  assert.equal(params.claudeSettings, undefined);
});

test("an API key account puts the key in env and only a helper command in claudeSettings", async (t) => {
  const { launcher, requests, cwd } = setup(t, {
    accounts: { claude: { a2: { kind: "api", value: API_KEY } } },
  });
  await launcher.launch(base(cwd, { agent: "claude", claudeAccountId: "a2" }));
  const { params } = requests[0];
  assert.deepEqual(Object.keys(params.claudeSettings), ["apiKeyHelper"]);
  assert.ok(!JSON.stringify(params.claudeSettings).includes(API_KEY));
  assert.equal(params.claudeSettings.env, undefined);
  const name = Object.keys(params.env).find(
    (key) => params.env[key] === API_KEY,
  );
  assert.ok(name && !name.startsWith("SUSHIAI_"));
  assert.ok(params.claudeSettings.apiKeyHelper.includes(`$${name}`));
});

test("a model profile sends its variables and key in env and a helper in claudeSettings", async (t) => {
  const { launcher, requests, cwd } = setup(t);
  await launcher.launch(base(cwd, { agent: "claude", modelProfileId: "m1" }));
  const { params } = requests[0];
  assert.equal(params.env.ANTHROPIC_BASE_URL, "https://models.invalid");
  assert.deepEqual(Object.keys(params.claudeSettings), ["apiKeyHelper"]);
  assert.ok(!JSON.stringify(params.claudeSettings).includes(MODEL_KEY));
  assert.ok(Object.values(params.env).includes(MODEL_KEY));
  await assert.rejects(
    launcher.launch(base(cwd, { agent: "codex", modelProfileId: "m1" })),
    /Claude sessions/,
  );
});

test("a Codex account sets an absolute CODEX_HOME and project values ride in env", async (t) => {
  const { launcher, requests, cwd } = setup(t, {
    project: { id: "p1" },
    accounts: { codex: { c1: { home: CODEX_HOME } } },
  });
  await launcher.launch(base(cwd, { agent: "codex", codexAccountId: "c1" }));
  const { params } = requests[0];
  assert.equal(params.env.CODEX_HOME, CODEX_HOME);
  assert.equal(params.env.PROJECT_VALUE, "pv");
  assert.equal(params.project, "p1");
  assert.equal(params.claudeSettings, undefined);
});

test("gemini and cursor-agent run as a plain command", async (t) => {
  const { launcher, requests, cwd } = setup(t);
  await launcher.launch(base(cwd, { agent: "gemini" }));
  assert.deepEqual(requests[0].params.cmd, ["gemini"]);
  await assert.rejects(
    launcher.launch(base(cwd, { agent: "vim" })),
    /Unsupported/,
  );
});

test("a worktree launch runs in the new checkout and a retry reuses it", async (t) => {
  const { launcher, requests, cwd } = setup(t);
  setup.worktrees.length = 0;
  const request = base(cwd, {
    worktree: { branch: "feat-x", base: "refs/heads/main" },
  });
  await launcher.launch(request);
  await launcher.launch(request);
  assert.equal(setup.worktrees.length, 1);
  assert.deepEqual(setup.worktrees[0].base, "refs/heads/main");
  assert.equal(requests[0].params.cwd, requests[1].params.cwd);
  assert.match(requests[0].params.cwd, /wt-feat-x$/);
  assert.equal(
    requests[0].params.idempotencyKey,
    requests[1].params.idempotencyKey,
  );
});

test("the renderer request keeps one key per launch and maps the workspace to group", () => {
  const request = {
    operationId: "op-1",
    endpoint: "/sock",
    kind: "agent",
    agent: "claude",
    cwd: "/repo",
    label: "Repo",
    workspaceId: "w1",
    claudeAccountId: "",
  };
  const first = toDaemonLaunch(request);
  assert.deepEqual(first, toDaemonLaunch(request));
  assert.equal(first.idempotencyKey, "op-1");
  assert.equal(first.host, "local");
  assert.equal(first.group, "w1");
  assert.equal(first.claudeAccountId, "");
  // One host name everywhere: the connection id without its `ssh:` prefix.
  assert.equal(toDaemonLaunch({ ...request, endpoint: "ssh:x" }).host, "x");
  assert.equal(toDaemonLaunch({ ...request, resume: "r-1" }).resume, "r-1");
  assert.equal(first.resume, undefined);
});

test("a remote host is refused and no error carries a secret", async (t) => {
  const { launcher, requests, cwd } = setup(t, {
    accounts: { claude: { a1: { kind: "subscription", value: TOKEN } } },
  });
  await assert.rejects(
    launcher.launch(
      base(cwd, { host: "ssh:box", agent: "claude", claudeAccountId: "a1" }),
    ),
    (error) =>
      error.code === "REMOTE_LAUNCH_LATER" && /later/.test(error.message),
  );
  await assert.rejects(
    sessionLaunchEnv({}, { host: "ssh:box", cwd, agent: "claude" }),
    /Remote launch comes later/,
  );
  const failing = createSessionLauncher({
    manager: {
      request: async () => {
        throw Object.assign(new Error("spawn failed"), { code: 5 });
      },
    },
    environment: async () => ({
      env: { CLAUDE_CODE_OAUTH_TOKEN: TOKEN },
      claudeSettings: {},
    }),
  });
  await assert.rejects(
    failing.launch(base(cwd, { agent: "claude" })),
    (error) => !error.message.includes(TOKEN),
  );
  assert.equal(requests.length, 0);
});

test("the IPC handler dispatches a validated launch to the launcher", async () => {
  const handlers = new Map();
  const seen = [];
  registerDaemonIpc({
    handle: (channel, fn) => handlers.set(channel, fn),
    launch: async (request) => {
      seen.push(request);
      return { host: "local", sessionId: "s9" };
    },
  });
  const request = base("/repo");
  assert.deepEqual(await handlers.get("daemon-session-launch")(request), {
    host: "local",
    sessionId: "s9",
  });
  assert.deepEqual(seen, [request]);
  await assert.rejects(
    handlers.get("daemon-session-launch")({ ...request, modelProfileId: 4 }),
    /Invalid modelProfileId/,
  );
});

// Real daemon smoke: a fake `claude` on PATH runs the hook commands from its
// --settings, so the agent status goes working, then idle.
const DAEMON = path.join(
  process.env.CARGO_TARGET_DIR || path.join(__dirname, "..", "target"),
  "debug",
  "sushiai",
);
const FIXTURES = path.join(
  __dirname,
  "..",
  "crates",
  "sushiai-agents",
  "tests",
  "fixtures",
  "claude",
);

const FAKE_CLAUDE = `#!/usr/bin/env node
const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const settings = JSON.parse(process.argv[process.argv.indexOf("--settings") + 1]);
const out = process.env.FAKE_OUT;
const hook = (event, file) =>
  spawnSync("sh", ["-c", settings.hooks[event][0].hooks[0].command], {
    input: fs.readFileSync(path.join(${JSON.stringify(FIXTURES)}, file)),
  });
fs.writeFileSync(path.join(out, "token"), process.env.CLAUDE_CODE_OAUTH_TOKEN || "");
hook("SessionStart", "SessionStart-startup.json");
hook("UserPromptSubmit", "UserPromptSubmit.json");
fs.writeFileSync(path.join(out, "working"), "x");
const wait = () => !fs.existsSync(path.join(out, "go"));
const timer = setInterval(() => {
  if (wait()) return;
  hook("Stop", "Stop.json");
  fs.writeFileSync(path.join(out, "stopped"), "x");
  clearInterval(timer);
  setTimeout(() => {}, 60000);
}, 50);
`;

async function until(label, check, seconds = 15) {
  const end = Date.now() + seconds * 1000;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

test(
  "real daemon: a launched fake claude is listed and goes working then idle without leaking the token",
  { skip: !fs.existsSync(DAEMON) && "debug daemon is not built" },
  async (t) => {
    const root = tempDir(t, "l4s-");
    const home = path.join(root, "h");
    const user = path.join(root, "u");
    const codexHome = path.join(root, "c");
    const bin = path.join(root, "bin");
    const out = path.join(root, "out");
    for (const dir of [home, user, codexHome, bin, out])
      fs.mkdirSync(dir, { mode: 0o700 });
    fs.writeFileSync(path.join(bin, "claude"), FAKE_CLAUDE, { mode: 0o755 });
    const env = {
      PATH: `${bin}:${process.env.PATH}`,
      SUSHIAI_HOME: home,
      HOME: user,
      CODEX_HOME: codexHome,
      FAKE_OUT: out,
    };
    const daemon = spawn(DAEMON, ["daemon"], { env, stdio: "ignore" });
    const socketPath = path.join(home, "daemon.sock");
    let client;
    t.after(async () => {
      try {
        if (client) {
          for (const session of await client
            .request("session.list", null)
            .catch(() => []))
            await client
              .request("session.close", { id: session.id, graceful: false })
              .catch(() => {});
          await client.request("daemon.shutdown", {}).catch(() => {});
          client.close();
        }
      } finally {
        await new Promise((resolve) => {
          if (daemon.exitCode !== null) return resolve();
          daemon.once("exit", resolve);
          daemon.kill("SIGTERM");
          setTimeout(resolve, 3000);
        });
        // Holders are their own sessions; stop any left under this home.
        spawnSync("pkill", ["-f", home]);
      }
    });
    const { connectDaemon } = require("../electron/daemon/client.cjs");
    client = await until("daemon socket", () =>
      connectDaemon({ socketPath, clientName: "l4a-test" }).catch(() => null),
    );
    const manager = {
      request: (host, method, params) => client.request(method, params),
    };
    const launcher = createSessionLauncher({
      manager,
      environment: async () => ({
        env: { CLAUDE_CODE_OAUTH_TOKEN: TOKEN, FAKE_OUT: out },
        claudeSettings: {},
      }),
    });
    const workdir = fs.realpathSync(root);
    const launch = base(workdir, {
      agent: "claude",
      title: "Smoke",
      group: "ws-smoke",
    });
    const { sessionId } = await launcher.launch(launch);
    const again = await launcher.launch(launch);
    assert.equal(again.sessionId, sessionId);
    const find = async () =>
      (await client.request("session.list", null)).find(
        (s) => s.id === sessionId,
      );
    assert.equal((await find()).group, "ws-smoke");
    await until(
      "working",
      async () => (await find())?.agentStatus === "working",
    );
    assert.equal(fs.readFileSync(path.join(out, "token"), "utf8"), TOKEN);
    fs.writeFileSync(path.join(out, "go"), "x");
    await until("idle", async () => (await find())?.agentStatus === "idle");
    await client.request("daemon.shutdown", {}).catch(() => {});
    await until(
      "daemon exit",
      () => daemon.exitCode !== null || daemon.signalCode,
      10,
    );
    const leaked = [];
    const walk = (dir) => {
      for (const name of fs.readdirSync(dir)) {
        const file = path.join(dir, name);
        const stat = fs.lstatSync(file);
        if (stat.isDirectory()) walk(file);
        else if (stat.isFile() && fs.readFileSync(file).includes(TOKEN))
          leaked.push(file);
      }
    };
    walk(home);
    const hits = leaked.filter((file) =>
      /state\.json$|daemon\.log$/.test(file),
    );
    assert.deepEqual(hits, []);
    assert.ok(fs.existsSync(path.join(home, "state.json")));
  },
);
