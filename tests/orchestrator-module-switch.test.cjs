// The enable flow (owner O1): turning the Orchestrator on registers its module
// on a host and restarts that host's daemon; turning it off unregisters. Fake
// manager and fake exec; the real local run is in orchestrator-daemon-real.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  OrchestratorHosts,
  OrchestratorService,
  createModuleSwitch,
} = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");

/** A fake world: a manager whose hosts serve `orch` only after a registered
 * daemon restart, a fake CLI runner and a fake ssh exec. */
function world({ local = [], remote = null, failExec } = {}) {
  const log = [];
  const registered = { local: false, box: false };
  const hostsSpec = { local: { capabilities: local } };
  if (remote) hostsSpec.box = { capabilities: remote };
  const manager = fakeDaemonManager({
    hosts: hostsSpec,
    onRetry: (host, m) => {
      // A restarted daemon serves the module exactly when the host is registered.
      if (restarting.has(host)) {
        restarting.delete(host);
        log.push(`restarted ${host}`);
        m.setState(host, {
          state: "ready",
          capabilities: registered[host] ? ["orch"] : [],
        });
      }
    },
  });
  const restarting = new Set();
  manager.handlers["daemon.shutdown"] = (_params, host) => {
    log.push(`shutdown ${host}`);
    restarting.add(host);
    manager.setState(host, { state: "offline" });
  };
  const runLocal = async (args) => {
    log.push(`local ${args.join(" ")}`);
    registered.local = args[1] === "register";
    return "";
  };
  const restartLocal = async () => {
    log.push("restart local");
    restarting.add("local");
    manager.setState("local", { state: "offline" });
  };
  const exec = async (host, command) => {
    log.push(`exec ${host}`);
    if (failExec) throw new Error(failExec);
    assert.match(command, /orch (register|unregister)/);
    registered.box = /orch register/.test(command);
    return "";
  };
  const moduleSwitch = createModuleSwitch({
    getManager: () => manager,
    runLocal,
    restartLocal,
    exec,
    settleMs: 1,
    attempts: 3,
    readyTimeoutMs: 500,
  });
  return { manager, log, registered, moduleSwitch, exec };
}

test("enabling the local host registers, restarts the daemon and ends with the module served", async () => {
  const w = world();
  await w.moduleSwitch.enable("local");
  assert.deepEqual(w.log, [
    "local orch register",
    "restart local",
    "restarted local",
  ]);
  assert.deepEqual(w.manager.states()[0].capabilities, ["orch"]);
  // A second enable on the same run does nothing.
  await w.moduleSwitch.enable("local");
  assert.equal(w.log.length, 3);
});

test("a daemon that already serves the module is registered but not restarted", async () => {
  const w = world({ local: ["orch"] });
  await w.moduleSwitch.enable("local");
  assert.deepEqual(w.log, ["local orch register"]);
});

test("disabling unregisters and restarts a daemon that serves the module", async () => {
  const w = world({ local: ["orch"] });
  w.registered.local = true;
  await w.moduleSwitch.disable("local");
  assert.deepEqual(w.log, [
    "local orch unregister",
    "restart local",
    "restarted local",
  ]);
  assert.deepEqual(w.manager.states()[0].capabilities, []);
});

test("disabling stops the tasks with an agent running first, and waits until they have ended", async () => {
  const w = world({ local: ["orch"] });
  w.registered.local = true;
  const status = { t1: "running", t2: "landing", t3: "done", t4: "waiting" };
  w.manager.handlers["orch.task.list"] = () =>
    Object.entries(status).map(([id, s]) => ({ id, status: s }));
  w.manager.handlers["orch.task.stop"] = ({ id }) => {
    w.log.push(`stop ${id}`);
    // The agent takes a moment to end: the task stays live for a while.
    setTimeout(() => (status[id] = "stopped"), 150);
  };
  await w.moduleSwitch.disable("local");
  assert.deepEqual(w.log, [
    "stop t1",
    "stop t2",
    "local orch unregister",
    "restart local",
    "restarted local",
  ]);
  assert.deepEqual(status, {
    t1: "stopped",
    t2: "stopped",
    t3: "done",
    t4: "waiting",
  });
});

test("disabling still goes on when the tasks cannot be listed", async () => {
  const w = world({ local: ["orch"] });
  w.registered.local = true;
  w.manager.handlers["orch.task.list"] = () => {
    throw new Error("module is not serving");
  };
  await w.moduleSwitch.disable("local");
  assert.deepEqual(w.log, [
    "local orch unregister",
    "restart local",
    "restarted local",
  ]);
});

test("a remote host registers over the exec path and restarts through daemon.shutdown", async () => {
  const w = world({ remote: [] });
  await w.moduleSwitch.enable("ssh:box");
  assert.deepEqual(w.log, ["exec ssh:box", "shutdown box", "restarted box"]);
  assert.deepEqual(
    w.manager.states().find((s) => s.host === "box").capabilities,
    ["orch"],
  );
  await w.moduleSwitch.disable("ssh:box");
  assert.deepEqual(w.log.slice(3), [
    "exec ssh:box",
    "shutdown box",
    "restarted box",
  ]);
});

test("a failed register rejects, and the next enable tries again", async () => {
  const w = world({ remote: [], failExec: "ssh is down" });
  await assert.rejects(w.moduleSwitch.enable("ssh:box"), /ssh is down/);
  await assert.rejects(w.moduleSwitch.enable("ssh:box"), /ssh is down/);
  assert.equal(w.log.filter((line) => line === "exec ssh:box").length, 2);
});

test("a remote service enables its host on the first request when the daemon lacks the module", async () => {
  const w = world({ remote: [] });
  w.manager.handlers["orch.task.list"] = () => [{ id: "t1" }];
  const service = new OrchestratorService({
    host: "ssh:box",
    getManager: () => w.manager,
    getConnections: () => ({ get: () => ({ name: "Box" }) }),
    send: () => {},
    moduleSwitch: w.moduleSwitch,
  });
  const tasks = await service.call("task.list", {});
  assert.equal(tasks[0].id, "t1");
  assert.deepEqual(w.log, ["exec ssh:box", "shutdown box", "restarted box"]);
});

test("a service whose enabling fails explains why", async () => {
  const w = world({ remote: [], failExec: "ssh is down" });
  const service = new OrchestratorService({
    host: "ssh:box",
    getManager: () => w.manager,
    getConnections: () => ({ get: () => ({ name: "Box" }) }),
    send: () => {},
    moduleSwitch: w.moduleSwitch,
  });
  await assert.rejects(
    service.call("task.list", {}),
    /Could not enable the orchestrator on Box: ssh is down/,
  );
});

test("the hosts toggle enables the Mac on, and disables the Mac and connected hosts off", async () => {
  const w = world({ local: [], remote: ["orch"] });
  const hosts = new OrchestratorHosts({
    local: { refreshSecrets: async () => {} },
    connections: () => ({ get: () => ({ name: "Box" }), list: () => [] }),
    getManager: () => w.manager,
    createService: () => ({ refreshSecrets: async () => {} }),
    moduleSwitch: w.moduleSwitch,
    enabled: false,
  });
  await hosts.setEnabled(true);
  assert.deepEqual(w.log, [
    "local orch register",
    "restart local",
    "restarted local",
  ]);
  hosts.services.set("ssh:box", {});
  w.log.length = 0;
  await hosts.setEnabled(false);
  for (const line of [
    "local orch unregister",
    "restarted local",
    "exec ssh:box",
    "restarted box",
  ])
    assert.ok(w.log.includes(line), line);
  assert.ok(w.log.includes("exec ssh:box"));
  assert.ok(w.log.includes("shutdown box"));
});

test("a toggle never fails because the module could not be switched", async () => {
  const messages = [];
  const hosts = new OrchestratorHosts({
    local: { refreshSecrets: async () => {} },
    connections: () => null,
    getManager: () => fakeDaemonManager(),
    createService: () => ({}),
    moduleSwitch: {
      enable: async () => {
        throw new Error("no binary");
      },
      disable: async () => {
        throw new Error("no binary");
      },
    },
    log: (message) => messages.push(message),
    enabled: false,
  });
  await hosts.setEnabled(true);
  await hosts.setEnabled(false);
  assert.deepEqual(messages, [
    "orchestrator enable on local: no binary",
    "orchestrator disable on local: no binary",
  ]);
});
