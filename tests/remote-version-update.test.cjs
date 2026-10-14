// A remote host on the bundled sushiai version connects without an update
// offer. Another version offers one without blocking, and only a daemon
// flagged for an update reads as "Update sushiai" in the orchestrator.
const test = require("node:test");
const assert = require("node:assert/strict");
const { watchHostTools } = require("../electron/daemon/host-tools.cjs");
const { createOrchestratorHosts } = require("../electron/orchestrator.cjs");

const SHA = "a".repeat(64);
const OTHER_BUILD = "b".repeat(64);
const BUNDLED = { sha256: SHA, version: "0.8.0" };
const hello = (version) => ({ host: "h1", version, build: OTHER_BUILD });

async function updateFlags(answer) {
  const listeners = [];
  const updates = [];
  watchHostTools({
    manager: {
      on: (name, callback) => {
        listeners.push(callback);
        return () => {};
      },
      hello: () => answer,
      setUpdate: (host, flag) => updates.push([host, flag]),
    },
    connections: { hasShell: () => true, exec: async () => "Linux x86_64" },
    store: { read: () => ({ h1: true }), write: () => {} },
    setup: async () => ({}),
    manifest: () => ({ manifest: { "Linux x86_64": BUNDLED } }),
  });
  listeners[0]({ host: "h1", state: "ready", generation: 1 });
  await new Promise((resolve) => setTimeout(resolve, 20));
  return updates;
}

test("a host on the bundled version is not flagged for update when only its build differs", async () => {
  const updates = await updateFlags(hello("0.8.0"));
  assert.equal(
    updates.some(([, flag]) => flag),
    false,
  );
});

test("a host on another version is flagged for update", async () => {
  assert.deepEqual(await updateFlags(hello("0.7.2")), [["h1", true]]);
});

test("a host on another version stays connected and offers Update sushiai", async () => {
  const { describeHost } = await import("../src/connectionState.ts");
  const view = describeHost({
    host: "h1",
    generation: 1,
    state: "ready",
    version: "0.7.2",
    update: true,
  });
  assert.equal(view.label, "Connected");
  assert.equal(view.tone, "info");
  assert.equal(view.action, "update");
});

const PROFILE = { id: "host-1", host: "user@devbox", name: "Devbox" };
const ENDPOINT = "ssh:host-1";
const ready = (extra) => ({
  host: PROFILE.id,
  state: "ready",
  generation: 1,
  version: "0.8.0",
  capabilities: ["sessions", "attach"],
  ...extra,
});

function orchestratorHosts(state, moduleSwitch) {
  const manager = {
    states: () => [state],
    on: () => () => {},
    retry: async () => state,
    request: async () => {
      throw new Error("no daemon request expected");
    },
  };
  const connections = {
    get: () => PROFILE,
    list: () => [PROFILE],
    hasShell: () => true,
    exec: async () => "",
  };
  return createOrchestratorHosts({
    send: () => {},
    notify: () => {},
    getConnections: () => connections,
    getManager: () => manager,
    moduleSwitch,
    enabled: true,
  });
}
const record = (hosts) => hosts.list().find((host) => host.id === ENDPOINT);

test("a host on the bundled version whose orchestrator is off is being turned on, not sent to Update", () => {
  const shown = record(orchestratorHosts(ready()));
  assert.equal(shown.state, "connecting");
  assert.match(shown.detail, /Turning on the orchestrator/);
  assert.doesNotMatch(shown.detail, /Update sushiai/);
});

test("a host flagged for update that has no orchestrator says Update sushiai", () => {
  const shown = record(orchestratorHosts(ready({ update: true })));
  assert.equal(shown.state, "error");
  assert.equal(shown.detail, "Update sushiai on Devbox");
});

test("a host on another version that has the orchestrator is ready: the update never blocks", () => {
  const capabilities = ["sessions", "attach", "orch"];
  const state = ready({ update: true, capabilities });
  assert.equal(record(orchestratorHosts(state)).state, "ready");
});

test("a failed turn-on shows its own reason, not Update sushiai", async () => {
  const moduleSwitch = {
    enable: async () => {
      throw new Error("orch register failed");
    },
    disable: async () => {},
  };
  const hosts = orchestratorHosts(ready(), moduleSwitch);
  await assert.rejects(
    hosts.call("task.list", {}, ENDPOINT),
    /Could not enable the orchestrator on Devbox/,
  );
  const shown = record(hosts);
  assert.equal(shown.state, "error");
  assert.match(shown.detail, /Could not enable the orchestrator on Devbox/);
  assert.doesNotMatch(shown.detail, /Update sushiai/);
});
