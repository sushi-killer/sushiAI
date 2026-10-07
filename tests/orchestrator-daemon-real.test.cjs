// The desktop orchestrator against the real `sushiai` daemon (debug build):
// the `orch` capability, an `orch.*` round trip through the daemon manager, and
// a daemon that is killed with SIGKILL and comes back. Everything runs in a
// temporary home; nothing of the owner's ~/.sushiai is touched.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const childProcess = require("node:child_process");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const { createLocalConnector } = require("../electron/daemon/local.cjs");
const {
  createOrchestratorHosts,
  createModuleSwitch,
} = require("../electron/orchestrator.cjs");

const binary = path.join(
  process.env.CARGO_TARGET_DIR || path.join(__dirname, "../target"),
  "debug/sushiai",
);
const until = async (check, what, ms = 15000) => {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
};
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** A temporary home, a local connector and a manager over it. `binDir` goes
 * first in the daemon's PATH (a fake harness); `enabled` writes the module's
 * flag before the first start. Everything is removed after the test. */
function world(t, { binDir = "", enabled = false } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "orch-real-"));
  const home = path.join(root, "home");
  fs.mkdirSync(home, { mode: 0o700 });
  if (enabled) {
    fs.mkdirSync(path.join(home, "modules"));
    fs.writeFileSync(path.join(home, "modules/orch.enabled"), "");
  }
  const daemonEnv = {
    PATH: [binDir, process.env.PATH].filter(Boolean).join(path.delimiter),
    HOME: root,
    CODEX_HOME: path.join(root, "codex"),
    SUSHIAI_HOME: home,
    SUSHIAI_DAEMON_BIN: binary,
    SUSHIAI_LOG: "off",
  };
  const connector = createLocalConnector({
    env: daemonEnv,
    appVersion: "0.0.0-test",
    startTimeoutMs: 20000,
    testMode: true,
  });
  const manager = createDaemonManager({
    connectors: { local: connector },
    backoffMinMs: 50,
    backoffMaxMs: 500,
    random: () => 1,
  });
  const pidOf = () =>
    Number(fs.readFileSync(path.join(home, "daemon.lock"), "utf8").trim());
  t.after(async () => {
    manager.close();
    const pid = pidOf();
    if (alive(pid)) process.kill(pid, "SIGTERM");
    await until(() => !alive(pid), "the daemon to stop").catch(() => {});
    fs.rmSync(root, { recursive: true, force: true });
  });
  const ready = [];
  manager.on("state", (state) => {
    if (state.state === "ready") ready.push(state.generation);
  });
  return { root, home, connector, manager, pidOf, ready };
}

test(
  "the local daemon offers orch, answers orch.*, and a kill -9 of it ends in a ready daemon with the secrets pushed again",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async (t) => {
    const { home, connector, manager, pidOf, ready } = world(t);
    const hosts = createOrchestratorHosts({
      send: () => {},
      getConnections: () => ({ get: () => ({}), list: () => [] }),
      getManager: () => manager,
      getProjects: () => ({
        agentEnvironments: async () => ({}),
        mcpEnvironments: async () => ({}),
      }),
      moduleSwitch: createModuleSwitch({
        getManager: () => manager,
        runLocal: (args) => connector.runCli(args),
        restartLocal: () => connector.restart(),
        exec: async () => {
          throw new Error("no ssh in this test");
        },
      }),
      enabled: false,
    });
    // What the daemon was asked on `orch.secrets.set`, seen from the host.
    const pushed = [];
    const request = manager.request.bind(manager);
    manager.request = (host, method, params, options) => {
      if (method === "orch.secrets.set" || method === "orch.settings.get")
        pushed.push(method);
      return request(host, method, params, options);
    };

    manager.start();
    await until(() => ready.length > 0, "the daemon to be ready");
    // A fresh daemon does not host the module: it is opt-in.
    const before = manager.states().find((item) => item.host === "local");
    assert.ok(!before.capabilities.includes("orch"), "no orch before enabling");
    const firstPid = pidOf();
    // Enabling registers it in the temp home and restarts the daemon.
    await hosts.setEnabled(true);
    assert.ok(fs.existsSync(path.join(home, "modules/orch.enabled")));
    assert.notEqual(pidOf(), firstPid, "the daemon restarted");

    const state = manager.states().find((item) => item.host === "local");
    assert.ok(state.capabilities.includes("orch"), "orch capability");
    assert.deepEqual(await hosts.probe("local"), { pid: 0 });
    assert.deepEqual(await manager.request("local", "orch.task.list", {}), []);
    // A method the module does not have is the module's own error.
    await assert.rejects(manager.request("local", "orch.nope", {}), {
      message: /unknown method orch\.nope/,
    });
    // Ready pushed the secrets: the first thing asked is the host's settings.
    await until(() => pushed.includes("orch.settings.get"), "the secrets push");

    // kill -9: no goodbye, no cleanup. The manager brings a daemon back.
    const doomed = pidOf();
    const pushes = pushed.length;
    process.kill(doomed, "SIGKILL");
    await until(() => !alive(doomed), "the old daemon to die");
    await manager.retry("local");
    await until(() => ready.length >= 2, "the daemon to be ready again");
    assert.notEqual(pidOf(), doomed);
    assert.deepEqual(await manager.request("local", "orch.task.list", {}), []);
    await until(() => pushed.length > pushes, "the secrets pushed again");

    // Turning it off unregisters and restarts without the module.
    await hosts.setEnabled(false);
    assert.ok(!fs.existsSync(path.join(home, "modules/orch.enabled")));
    const off = manager.states().find((item) => item.host === "local");
    assert.ok(!off.capabilities.includes("orch"), "no orch after disabling");
  },
);

const INIT = '{"type":"system","subtype":"init","session_id":"sess-fake"}';
const MESSAGE =
  '{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":100}},"parent_tool_use_id":null,"session_id":"sess-fake"}';
const RESULT =
  '{"type":"result","total_cost_usd":0.3,"usage":{"input_tokens":1,"output_tokens":1},"result":"done"}';

test(
  "with the module enabled, a task run survives a kill -9 of the daemon and finishes with one attempt",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async (t) => {
    // A fake `claude` first in the daemon's PATH: it streams one message,
    // waits, then reports its cost and exits 0.
    const bin = fs.mkdtempSync(path.join(os.tmpdir(), "orch-fake-bin-"));
    t.after(() => fs.rmSync(bin, { recursive: true, force: true }));
    fs.writeFileSync(
      path.join(bin, "claude"),
      `#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\necho '${INIT}'\necho '${MESSAGE}'\nsleep 4\necho '${RESULT}'\nexit 0\n`,
      { mode: 0o755 },
    );
    const { root, home, manager, pidOf, ready } = world(t, {
      binDir: bin,
      enabled: true,
    });
    fs.writeFileSync(
      path.join(root, ".gitconfig"),
      "[user]\n\tname = orch test\n\temail = orch-test@example.invalid\n",
    );
    const repo = path.join(root, "repo");
    fs.mkdirSync(repo);
    const git = (...args) =>
      childProcess.execFileSync("git", args, { cwd: repo, stdio: "ignore" });
    git("init", "-q");
    fs.writeFileSync(path.join(repo, "README.md"), "hello\n");
    git("add", ".");
    git(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.invalid",
      "commit",
      "-qm",
      "init",
    );

    manager.start();
    await until(() => ready.length > 0, "the daemon to be ready");
    const settings = await manager.request("local", "orch.settings.get", {});
    await manager.request("local", "orch.settings.set", {
      settings: {
        ...settings,
        review: "",
        briefCheckRoute: "",
        answerPolicy: false,
        sandbox: "host",
      },
    });
    const task = await manager.request("local", "orch.task.create", {
      repo,
      title: "Survives",
      goal: "g",
      criteria: [],
      verify: ["true"],
    });
    t.after(() => fs.rmSync(task.worktree, { recursive: true, force: true }));
    const get = () =>
      manager.request("local", "orch.task.get", { id: task.id });
    const events = path.join(
      home,
      "orchestrator/tasks",
      task.id,
      "runs/1/events.jsonl",
    );
    const running = await until(async () => {
      const current = await get();
      const seen =
        fs.existsSync(events) &&
        fs.readFileSync(events, "utf8").includes("msg_1");
      return seen && current.attempts[0]?.pgid ? current : null;
    }, "the run to stream");
    const pgid = running.attempts[0].pgid;

    // kill -9 mid-run: the run is the daemon's child in its own group and
    // goes on without it.
    const daemon = pidOf();
    process.kill(daemon, "SIGKILL");
    await until(() => !alive(daemon), "the daemon to die");
    assert.ok(alive(pgid), "the run outlives the daemon");
    await manager.retry("local");
    await until(() => ready.length >= 2, "the daemon to be ready again");

    const done = await until(
      async () => {
        const current = await get();
        return current.status === "done" ? current : null;
      },
      "the task to finish",
      30000,
    );
    assert.equal(done.attempts.length, 1, "no requeue");
    assert.equal(done.attempts[0].status, "passed");
    assert.equal(done.attempts[0].costUsd, 0.3);
  },
);

test(
  "turning the Orchestrator off with a task running and another queued leaves no agent process behind",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async (t) => {
    // A fake `claude` that streams one message and then waits for a minute.
    const bin = fs.mkdtempSync(path.join(os.tmpdir(), "orch-fake-bin-"));
    t.after(() => fs.rmSync(bin, { recursive: true, force: true }));
    fs.writeFileSync(
      path.join(bin, "claude"),
      `#!/bin/sh\necho started >> ${bin}/starts\ncat > /dev/null\necho '${INIT}'\necho '${MESSAGE}'\nsleep 60\n`,
      { mode: 0o755 },
    );
    const { root, home, connector, manager, ready } = world(t, {
      binDir: bin,
      enabled: true,
    });
    fs.writeFileSync(
      path.join(root, ".gitconfig"),
      "[user]\n\tname = orch test\n\temail = orch-test@example.invalid\n",
    );
    const repo = path.join(root, "repo");
    fs.mkdirSync(repo);
    const git = (...args) =>
      childProcess.execFileSync("git", args, { cwd: repo, stdio: "ignore" });
    git("init", "-q");
    fs.writeFileSync(path.join(repo, "README.md"), "hello\n");
    git("add", ".");
    git(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.invalid",
      "commit",
      "-qm",
      "init",
    );
    const hosts = createOrchestratorHosts({
      send: () => {},
      getConnections: () => ({ get: () => ({}), list: () => [] }),
      getManager: () => manager,
      getProjects: () => ({
        agentEnvironments: async () => ({}),
        mcpEnvironments: async () => ({}),
      }),
      moduleSwitch: createModuleSwitch({
        getManager: () => manager,
        runLocal: (args) => connector.runCli(args),
        restartLocal: () => connector.restart(),
        exec: async () => {
          throw new Error("no ssh in this test");
        },
      }),
      enabled: false,
    });

    manager.start();
    await until(() => ready.length > 0, "the daemon to be ready");
    await hosts.setEnabled(true);
    const settings = await manager.request("local", "orch.settings.get", {});
    await manager.request("local", "orch.settings.set", {
      settings: {
        ...settings,
        review: "",
        briefCheckRoute: "",
        answerPolicy: false,
        sandbox: "host",
        parallel: 1,
      },
    });
    // One slot (read at daemon start): the second task waits behind the first.
    await connector.restart();
    await until(() => ready.length > 1, "the daemon to restart");
    const task = await manager.request("local", "orch.task.create", {
      repo,
      title: "Stops with the module",
      goal: "g",
      criteria: [],
      verify: ["true"],
    });
    t.after(() => fs.rmSync(task.worktree, { recursive: true, force: true }));
    const queued = await manager.request("local", "orch.task.create", {
      repo,
      title: "Waits behind the first",
      goal: "g",
      criteria: [],
      verify: ["true"],
    });
    t.after(() => fs.rmSync(queued.worktree, { recursive: true, force: true }));
    const events = path.join(
      home,
      "orchestrator/tasks",
      task.id,
      "runs/1/events.jsonl",
    );
    const running = await until(async () => {
      const current = await manager.request("local", "orch.task.get", {
        id: task.id,
      });
      const seen =
        fs.existsSync(events) &&
        fs.readFileSync(events, "utf8").includes("msg_1");
      return seen && current.attempts[0]?.pgid ? current : null;
    }, "the run to stream");
    const pgid = running.attempts[0].pgid;
    t.after(() => {
      try {
        process.kill(-pgid, "SIGKILL");
      } catch {}
    });

    await hosts.setEnabled(false);
    await until(() => !alive(pgid), "the agent to end", 5000);
    // The queued task never got its turn: one agent ever started.
    await new Promise((resolve) => setTimeout(resolve, 1000));
    assert.equal(
      fs.readFileSync(path.join(bin, "starts"), "utf8").trim(),
      "started",
    );
  },
);
