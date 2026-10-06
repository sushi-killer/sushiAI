// The desktop orchestrator against the real `sushiai` daemon (debug build):
// the `orch` capability, an `orch.*` round trip through the daemon manager, and
// a daemon that is killed with SIGKILL and comes back. Everything runs in a
// temporary home; nothing of the owner's ~/.sushiai is touched.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const { createLocalConnector } = require("../electron/daemon/local.cjs");
const { createOrchestratorHosts } = require("../electron/orchestrator.cjs");

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

test(
  "the local daemon offers orch, answers orch.*, and a kill -9 of it ends in a ready daemon with the secrets pushed again",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async (t) => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "orch-real-"));
    const home = path.join(root, "home");
    fs.mkdirSync(home, { mode: 0o700 });
    const env = {
      PATH: process.env.PATH,
      HOME: root,
      CODEX_HOME: path.join(root, "codex"),
      SUSHIAI_HOME: home,
      SUSHIAI_DAEMON_BIN: binary,
      SUSHIAI_LOG: "off",
    };
    const manager = createDaemonManager({
      connectors: {
        local: createLocalConnector({
          env,
          appVersion: "0.0.0-test",
          startTimeoutMs: 20000,
        }),
      },
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
    const hosts = createOrchestratorHosts({
      send: () => {},
      getConnections: () => ({ get: () => ({}), list: () => [] }),
      getManager: () => manager,
      getProjects: () => ({
        agentEnvironments: async () => ({}),
        mcpEnvironments: async () => ({}),
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
    await hosts.setEnabled(true);

    const state = manager.states().find((item) => item.host === "local");
    assert.ok(state.capabilities.includes("orch"), "orch capability");
    assert.deepEqual(await hosts.probe("local"), { pid: 0 });
    assert.deepEqual(await manager.request("local", "orch.echo", { n: 1 }), {
      echo: { n: 1 },
    });
    // A method the module does not have is the module's own error.
    await assert.rejects(manager.request("local", "orch.nope", {}), {
      message: /unknown method orch\.nope/,
    });
    // Ready pushed the secrets: the first thing asked is the host's settings.
    await until(() => pushed.includes("orch.settings.get"), "the secrets push");

    // kill -9: no goodbye, no cleanup. The manager brings a daemon back.
    const before = pidOf();
    const pushes = pushed.length;
    process.kill(before, "SIGKILL");
    await until(() => !alive(before), "the old daemon to die");
    await manager.retry("local");
    await until(() => ready.length >= 2, "the daemon to be ready again");
    assert.notEqual(pidOf(), before);
    assert.deepEqual(await manager.request("local", "orch.echo", { n: 2 }), {
      echo: { n: 2 },
    });
    await until(() => pushed.length > pushes, "the secrets pushed again");
  },
);
