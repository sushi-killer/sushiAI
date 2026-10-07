const { test } = require("node:test");
const assert = require("node:assert/strict");
const { registerDaemonIpc } = require("../electron/ipc/daemon.cjs");

const launched = [];
const handlers = new Map();
registerDaemonIpc({
  handle: (channel, run) => handlers.set(channel, run),
  send: () => {},
  getManager: () => null,
  launch: async (request) => launched.push(request),
  installHost: async (host) => ({ installed: host }),
});

test("without a manager, daemonStates is empty and the rest reject", async () => {
  assert.deepEqual(await handlers.get("daemon-states")(), []);
  // Launch and host-install go to the injected functions, manager or not.
  assert.deepEqual(await handlers.get("host-install")("local"), {
    installed: "local",
  });
  await assert.rejects(
    handlers.get("daemon-terminal-ack")("panel", 10),
    /daemon manager is not running/,
  );
});

test("handlers reject malformed arguments before anything else", async () => {
  const launch = {
    host: "local",
    cwd: "/work",
    cols: 80,
    rows: 24,
    idempotencyKey: "k1",
  };
  const bad = [
    ["daemon-sessions-list", []],
    ["daemon-sessions-list", [7]],
    ["daemon-terminal-attach-data", ["p1", "a.png", "not bytes"]],
    ["daemon-terminal-attach-data", ["p1", "", new Uint8Array(1)]],
    ["daemon-session-launch", [null]],
    ["daemon-session-launch", [{ ...launch, cols: 1.5 }]],
    ["daemon-session-launch", [{ ...launch, idempotencyKey: "" }]],
    ["daemon-session-launch", [{ ...launch, extraArgs: [1] }]],
    ["daemon-session-close", ["local", "s1", "yes"]],
    ["daemon-session-update", ["local", {}]],
    ["daemon-session-read", ["local", "s1", -1]],
    ["daemon-session-input", ["local", "s1", 5]],
    ["daemon-ask-respond", ["local", { sessionId: "s", askId: "a" }]],
    ["daemon-terminal-attach", [{ panelId: "p", host: "local" }]],
    ["daemon-terminal-write", ["", "x"]],
    ["daemon-terminal-resize", ["p", 80, Number.NaN]],
    ["daemon-terminal-detach", [undefined]],
    ["daemon-terminal-ack", ["p", Infinity]],
    ["daemon-terminal-attach-file", ["p", ""]],
    ["host-install", [""]],
  ];
  for (const [channel, args] of bad)
    await assert.rejects(
      handlers.get(channel)(...args),
      (error) => /^Invalid /.test(error.message),
      `${channel} ${JSON.stringify(args)}`,
    );
  assert.equal(launched.length, 0, "a bad request never reached the launcher");
  await handlers.get("daemon-session-launch")(launch);
  assert.deepEqual(launched, [launch]);
});
