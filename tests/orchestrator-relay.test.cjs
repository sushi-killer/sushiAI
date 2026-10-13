const test = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { createOrchestratorHosts } = require("../electron/orchestrator.cjs");
const { until } = require("./helpers/deadline.cjs");

// The app's side of the MCP bridge's `host` argument: a daemon asks for
// another host's orchestrator with an `orch.relay` notification and the app
// answers with `orch.relay.reply` over the same daemon connection.

const LAB = {
  id: "lab",
  name: "Lab",
  host: "dev@lab.example.test",
  identityFile: "/home/dev/.ssh/lab_key",
};
const DOWN = { id: "down", name: "Down", host: "dev@down.example.test" };
const PROFILES = [LAB, DOWN];

/** `answers` maps "<manager host>:<method>" to what that daemon returns. */
async function rig(answers = {}) {
  const bus = new EventEmitter();
  const requests = [];
  const manager = {
    on(name, listener) {
      bus.on(name, listener);
      return () => bus.off(name, listener);
    },
    states: () => [
      { host: "local", state: "ready", capabilities: ["orch"] },
      { host: LAB.id, state: "ready", capabilities: ["orch"] },
    ],
    retry: async () => ({ host: DOWN.id, state: "offline" }),
    request: async (host, method, params) => {
      requests.push({ host, method, params });
      return answers[`${host}:${method}`] ?? {};
    },
  };
  const connections = {
    get(host) {
      const profile = PROFILES.find((item) => `ssh:${item.id}` === host);
      if (!profile) throw new Error("unknown host");
      return profile;
    },
    list: () => PROFILES,
    hasShell: () => false,
  };
  const hosts = createOrchestratorHosts({
    send: () => {},
    notify: () => {},
    getConnections: () => connections,
    getManager: () => manager,
    enabled: false,
  });
  await hosts.setEnabled(true);
  const relay = (origin, id, host, method, params = {}) =>
    bus.emit("event", {
      host: origin,
      generation: 1,
      method: "orch.relay",
      params: { id, host, method, params },
    });
  const replyTo = async (id) => {
    const asked = () =>
      requests.find(
        (item) => item.method === "orch.relay.reply" && item.params.id === id,
      );
    await until(() => asked(), `the reply to ${id}`);
    return asked();
  };
  return { requests, relay, replyTo };
}

test("a relayed task.list goes through that host's daemon and comes back labelled", async () => {
  const { requests, relay, replyTo } = await rig({
    "lab:orch.task.list": [{ id: "t1", status: "running" }],
  });
  // Asked by lab's own daemon, for the host lab.
  relay("lab", "r1", "ssh:lab", "task.list");
  const answer = await replyTo("r1");
  assert.equal(answer.host, "lab");
  assert.deepEqual(answer.params, {
    id: "r1",
    result: [{ id: "t1", status: "running", host: "ssh:lab" }],
  });
  const asked = requests.filter((item) => item.method === "orch.task.list");
  assert.deepEqual(
    asked.map((item) => item.host),
    ["lab"],
  );
});

test("local results and inbox messages are labelled too", async () => {
  const { relay, replyTo } = await rig({
    "local:orch.task.get": { id: "a", status: "done" },
    "lab:orch.message.inbox": [{ id: "m1", from: "t1", text: "hi" }],
  });
  relay("local", "r1", "local", "task.get", { id: "a" });
  const task = await replyTo("r1");
  assert.deepEqual(task.params.result, {
    id: "a",
    status: "done",
    host: "local",
  });
  relay("local", "r2", "ssh:lab", "message.inbox", { id: "orchestrator" });
  const inbox = await replyTo("r2");
  assert.deepEqual(inbox.params.result, [
    { id: "m1", from: "t1", text: "hi", host: "ssh:lab" },
  ]);
});

test("host all lists every saved host, labelled, and names the ones it could not reach", async () => {
  const { relay, replyTo } = await rig({
    "local:orch.task.list": [{ id: "a", status: "done" }],
    "lab:orch.task.list": [{ id: "b", status: "running" }],
  });
  relay("local", "r1", "all", "task.list");
  const answer = await replyTo("r1");
  const { results, unreachable } = answer.params.result;
  assert.deepEqual(results.map((task) => [task.id, task.host]).sort(), [
    ["a", "local"],
    ["b", "ssh:lab"],
  ]);
  assert.deepEqual(
    unreachable.map((item) => item.host),
    ["ssh:down"],
  );
});

test("hosts.list answers id, name and state only", async () => {
  const { relay, replyTo } = await rig();
  relay("local", "r1", "local", "hosts.list");
  const answer = await replyTo("r1");
  assert.deepEqual(answer.params.result, [
    { id: "local", name: "Local", state: "ready" },
    { id: "ssh:lab", name: "Lab", state: "ready" },
    { id: "ssh:down", name: "Down", state: "idle" },
  ]);
  const text = JSON.stringify(answer);
  assert.equal(text.includes("example.test"), false);
  assert.equal(text.includes("lab_key"), false);
});

test("only the task and message methods are served, and an unknown host is an error reply", async () => {
  const { requests, relay, replyTo } = await rig();
  const refused = [
    "settings.set",
    "secrets.set",
    "task.delete",
    "chat.send",
    "hook.stop",
  ];
  for (const [index, method] of refused.entries()) {
    const id = `x${index}`;
    relay("local", id, "local", method, {});
    const answer = await replyTo(id);
    assert.equal(typeof answer.params.error, "string", method);
    assert.equal("result" in answer.params, false, method);
    assert.equal(
      requests.some((item) => item.method === `orch.${method}`),
      false,
      method,
    );
  }
  relay("local", "ghost", "ssh:ghost", "task.list");
  const answer = await replyTo("ghost");
  assert.equal(typeof answer.params.error, "string");
});
