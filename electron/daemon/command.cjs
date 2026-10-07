"use strict";

// command connector: runs the argv of the profile (`connector: {kind:
// "command", argv}`) and speaks the daemon protocol on its stdio, the same
// byte pipe the ssh connector uses. The command has no status protocol, so
// the manager pings it every 15 s with a 10 s timeout (`$/ping`).

const {
  spawnPipe,
  connectOverPipe,
  failure,
  tail,
} = require("./connectors.cjs");

const { daemonEnv, resolveHome } = require("./local.cjs");

const HELLO_TIMEOUT_MS = 20000;
const PING_INTERVAL_MS = 15000;
const PING_TIMEOUT_MS = 10000;
const DAEMON_DIED = 2;

function describeExit({ code, signal, stderr = "", error } = {}) {
  const detail = tail(stderr) || error?.message || "";
  if (error?.code === "ENOENT")
    return {
      state: "failed",
      message: `Cannot run the connector command: ${error.message}`,
    };
  if (code === DAEMON_DIED)
    return {
      state: "failed",
      reason: "daemon_died",
      retryNow: true,
      message: "The daemon stopped.",
    };
  return {
    state: "failed",
    message:
      detail ||
      (signal
        ? `The connector command ended with ${signal}`
        : `The connector command exited with ${code}`),
  };
}

function createCommandConnector({
  profile,
  spawnProcess = spawnPipe,
  helloTimeoutMs = HELLO_TIMEOUT_MS,
  pingIntervalMs = PING_INTERVAL_MS,
  pingTimeoutMs = PING_TIMEOUT_MS,
}) {
  const [command, ...args] = profile.connector.argv;
  return {
    kind: "command",
    signature: JSON.stringify(["command", profile.connector.argv]),
    ping: { intervalMs: pingIntervalMs, timeoutMs: pingTimeoutMs },
    connect() {
      return connectOverPipe({
        pipe: spawnProcess(command, args, {
          env: daemonEnv(process.env, resolveHome()),
        }),
        helloTimeoutMs,
        classify(exit) {
          const { message, ...rest } = describeExit(exit);
          return failure(message, rest);
        },
      });
    },
    describeLoss(client) {
      const exit = client.exit?.();
      if (!exit) return null;
      const lost = describeExit(exit);
      return {
        ...lost,
        state: "offline",
        reason: lost.reason || "connection_lost",
      };
    },
  };
}

module.exports = { createCommandConnector, describeExit };
