const test = require("node:test");
const assert = require("node:assert/strict");
const { AgentRegistry } = require("../electron/agents/registry.cjs");

test("providers use the same contract without exposing transport access", async () => {
  const registry = new AgentRegistry();
  const make = (id) => ({ descriptor: { apiVersion: 1, id, capabilities: ["conversations"] },
    operations: new Map([["agents.list", async () => [{ id: "one", providerId: id }]]]),
    close: async () => {} });
  registry.register(make("hermes"));
  registry.register(make("alternative"));
  assert.equal(registry.list().length, 2);
  assert.equal((await registry.call("alternative", "agents.list"))[0].providerId, "alternative");
  await assert.rejects(registry.call("hermes", "rpc", { method: "anything" }), /not supported/);
  await assert.rejects(registry.call("missing", "agents.list"), /not installed/);
  await assert.rejects(registry.call("hermes", "agents.list", []), /object/);
  await registry.close();
  await assert.rejects(registry.call("hermes", "agents.list"), /closed/);
});

test("provider event identity cannot spoof another provider; closing releases resources", async () => {
  const registry = new AgentRegistry();
  let closed = 0;
  const provider = { descriptor: { apiVersion: 1, id: "one" }, operations: new Map(), close: () => closed++ };
  registry.register(provider);
  let received;
  registry.on("event", (event) => received = event);
  provider.publish({ providerId: "spoof", type: "changed" });
  assert.equal(received.providerId, "one");
  assert.throws(() => registry.register(provider), /already installed/);
  await registry.close();
  await registry.close();
  assert.equal(closed, 1);
});
