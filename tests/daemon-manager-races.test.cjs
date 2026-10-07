const test = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");

const tick = () => new Promise((resolve) => setImmediate(resolve));

// Connector whose connect() waits until the test settles it; counts clients
// and the ping intervals that are still live.
function harness() {
  const clients = [];
  const waiting = [];
  let liveIntervals = 0;
  const realSet = global.setInterval;
  const realClear = global.clearInterval;
  const handles = new Set();
  global.setInterval = (...args) => {
    const timer = realSet(...args);
    handles.add(timer);
    liveIntervals += 1;
    return timer;
  };
  global.clearInterval = (timer) => {
    if (handles.delete(timer)) liveIntervals -= 1;
    return realClear(timer);
  };
  const connector = {
    kind: "ssh",
    signature: "s",
    auto: false,
    ping: { intervalMs: 100000, timeoutMs: 1000 },
    connect: () =>
      new Promise((resolve, reject) =>
        waiting.push({
          ok: () => {
            const client = new EventEmitter();
            client.hello = { daemon: "1", host: "h", capabilities: [] };
            client.request = async () => ({});
            client.closed = false;
            client.close = () => {
              client.closed = true;
            };
            clients.push(client);
            resolve(client);
          },
          fail: () => reject(new Error("boom")),
        }),
      ),
  };
  const manager = createDaemonManager({
    connectors: { h: connector },
    backoffMinMs: 5,
  });
  const restore = () => {
    manager.close();
    for (const timer of handles) realClear(timer);
    global.setInterval = realSet;
    global.clearInterval = realClear;
  };
  return {
    manager,
    clients,
    waiting,
    restore,
    live: () => liveIntervals,
  };
}

test("disconnect during a pending connect ends offline with the client closed", async () => {
  const h = harness();
  try {
    h.manager.start();
    const pending = h.manager.retry("h");
    await tick();
    h.manager.disconnect("h");
    h.waiting.shift().ok();
    await pending;
    const [state] = h.manager.states();
    assert.equal(state.state, "offline");
    assert.equal(state.reason, "disconnected");
    assert.equal(h.clients.length, 1);
    assert.equal(h.clients[0].closed, true);
    assert.equal(h.live(), 0);
  } finally {
    h.restore();
  }
});

test("disconnect then connect opens exactly one live client", async () => {
  const h = harness();
  try {
    h.manager.start();
    const first = h.manager.retry("h");
    await tick();
    h.manager.disconnect("h");
    const second = h.manager.retry("h");
    await tick();
    assert.equal(h.waiting.length, 2);
    h.waiting.splice(0).forEach((w) => w.ok());
    await Promise.all([first, second]);
    assert.equal(h.manager.states()[0].state, "ready");
    assert.equal(h.clients.length, 2);
    assert.equal(h.clients.filter((c) => !c.closed).length, 1);
    assert.equal(h.live(), 1);
  } finally {
    h.restore();
  }
});

test("a failed attempt after disconnect schedules no reconnect", async () => {
  const h = harness();
  try {
    h.manager.start();
    const pending = h.manager.retry("h");
    await tick();
    h.manager.disconnect("h");
    h.waiting.shift().fail();
    await pending;
    await new Promise((resolve) => setTimeout(resolve, 60));
    assert.equal(h.waiting.length, 0);
    assert.equal(h.manager.states()[0].reason, "disconnected");
  } finally {
    h.restore();
  }
});

test("a loss that is being described does not overwrite a disconnect", async () => {
  let finish;
  let live;
  const connector = {
    kind: "ssh",
    signature: "s",
    describeLoss: () => new Promise((resolve) => (finish = resolve)),
    connect: async () => {
      live = new EventEmitter();
      live.hello = { daemon: "1", host: "x", capabilities: [] };
      live.request = async () => ({});
      live.close = () => {};
      return live;
    },
  };
  const manager = createDaemonManager({ connectors: { x: connector } });
  try {
    manager.start();
    while (manager.states()[0].state !== "ready") await tick();
    live.emit("disconnect");
    await tick();
    manager.disconnect("x");
    finish({ state: "offline", reason: "daemon_died", message: "lost" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(manager.states()[0].reason, "disconnected");
  } finally {
    manager.close();
  }
});
