const test = require("node:test");
const assert = require("node:assert/strict");
const {
  connectThroughDaemon,
} = require("../electron/daemon/connect-through.cjs");
const { serializePerHost } = require("../electron/daemon/host-tools.cjs");

test("a Disconnect during the connect retry leaves auto-connect off", async () => {
  let auto = null;
  let release;
  const states = [{ host: "box", state: "offline" }];
  const manager = {
    states: () => states,
    retry: () => new Promise((resolve) => (release = resolve)),
  };
  const connections = {
    setAutoConnect: async (_endpoint, value) => {
      auto = value;
    },
  };
  const connect = connectThroughDaemon({
    daemonHost: (e) => e.slice(4),
    getManager: () => manager,
    getConnections: () => connections,
  })(async () => "original");
  const pending = connect("ssh:box");
  await new Promise((resolve) => setImmediate(resolve));
  // The user clicks Disconnect while the retry is pending.
  await connections.setAutoConnect("ssh:box", false);
  release({ host: "box", state: "ready" });
  assert.deepEqual(await pending, { connected: true, setup: "" });
  assert.equal(auto, false);
});

test("a host the manager does not own goes to the original connect", async () => {
  const connect = connectThroughDaemon({
    daemonHost: () => null,
    getManager: () => null,
    getConnections: () => null,
  })(async (endpoint) => `orig:${endpoint}`);
  assert.equal(await connect("ssh:x"), "orig:ssh:x");
});

test("setup for one host never overlaps, other hosts run in parallel", async () => {
  let active = 0;
  let peak = 0;
  const releases = [];
  const setup = serializePerHost(
    (endpoint) =>
      new Promise((resolve) => {
        active += 1;
        peak = Math.max(peak, active);
        releases.push(() => {
          active -= 1;
          resolve(endpoint);
        });
      }),
  );
  const a = setup("ssh:a", {});
  const b = setup("ssh:a", {});
  const other = setup("ssh:b", {});
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(releases.length, 2, "one for a, one for b");
  releases.shift()();
  await a;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(releases.length, 2, "b's second call starts after the first");
  releases.splice(0).forEach((release) => release());
  await Promise.all([b, other]);
  assert.equal(peak, 2);
});
