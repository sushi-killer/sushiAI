const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const {
  registerDaemonIpc,
  NOT_IMPLEMENTED,
} = require("../electron/ipc/daemon.cjs");

const preload = fs.readFileSync(
  path.join(__dirname, "../electron/preload.cjs"),
  "utf8",
);
const handlers = new Map();
registerDaemonIpc({ handle: (channel, run) => handlers.set(channel, run) });

test("every daemon channel in preload has a registered handler and back", () => {
  const exposed = [
    ...preload.matchAll(/invoke\("((?:daemon-|host-)[a-z-]+)"\)/g),
  ]
    .map((match) => match[1])
    .sort();
  assert.ok(exposed.length >= 16);
  assert.deepEqual(exposed, [...handlers.keys()].sort());
});

test("preload subscribes to the daemon events", () => {
  for (const event of ["daemon-state", "daemon-event", "daemon-terminal-data"])
    assert.ok(preload.includes(`ipcRenderer.on("${event}"`), event);
});

test("stubs answer daemonStates and reject everything else", async () => {
  assert.deepEqual(await handlers.get("daemon-states")(), []);
  await assert.rejects(
    handlers.get("host-install")("local"),
    new RegExp(NOT_IMPLEMENTED),
  );
  await assert.rejects(
    handlers.get("daemon-terminal-ack")("panel", 10),
    new RegExp(NOT_IMPLEMENTED),
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
    ["daemon-session-launch", [null]],
    ["daemon-session-launch", [{ ...launch, cols: 1.5 }]],
    ["daemon-session-launch", [{ ...launch, idempotencyKey: "" }]],
    ["daemon-session-launch", [{ ...launch, extraArgs: [1] }]],
    ["daemon-session-close", ["local", "s1", "yes"]],
    ["daemon-session-remove", ["local"]],
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
      (error) => !error.message.includes(NOT_IMPLEMENTED),
      `${channel} ${JSON.stringify(args)}`,
    );
  await assert.rejects(
    handlers.get("daemon-session-launch")(launch),
    new RegExp(NOT_IMPLEMENTED),
  );
});
