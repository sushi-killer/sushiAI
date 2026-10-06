"use strict";

// Remote connectors. A connector is { kind, signature, connect(), describeLoss?,
// ping? } (see manager.cjs): `connect()` yields a connected, hello-checked
// client, or rejects with an Error that may carry
//   state     "failed" (default) | "need_auth"
//   reason    "host_key_changed" | "not_installed" | "incompatible" | ...
//   retry     false = never retry on its own (the user has to act)
//   retryNow  true  = reconnect at once (the proxy reported "daemon died")
// ssh.cjs and command.cjs build the two kinds on the stdio pipe below; the
// local connector (local.cjs) stays separate because it owns a daemon.

const { Duplex } = require("node:stream");
const { spawn } = require("node:child_process");
const { connectDaemon } = require("./client.cjs");

const STDERR_TAIL = 4000;
const EXIT_GRACE_MS = 3000;

/** A child's stdout/stdin as the socket client.cjs expects. The child's exit
 * is recorded in `pipe.exit` ({code, signal, stderr}) before the stream
 * closes, so a listener of "disconnect" can read why. Destroying the stream
 * ends the child. */
function spawnPipe(command, args, options = {}) {
  const child = spawn(command, args, {
    stdio: ["pipe", "pipe", "pipe"],
    ...options,
  });
  let stderr = "";
  let done;
  const exited = new Promise((resolve) => (done = resolve));
  const pipe = new Duplex({
    read() {},
    write(chunk, _encoding, callback) {
      if (child.stdin.destroyed) return callback();
      child.stdin.write(chunk, () => callback());
    },
    final(callback) {
      child.stdin.end();
      callback();
    },
    destroy(error, callback) {
      if (child.exitCode === null && child.signalCode === null)
        child.kill("SIGTERM");
      callback(error);
    },
  });
  pipe.connecting = false;
  pipe.exit = null;
  pipe.exited = exited;
  pipe.child = child;
  child.stdin.on("error", () => {});
  child.stdout.on("data", (chunk) => pipe.push(chunk));
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-STDERR_TAIL);
  });
  child.once("error", (error) => {
    pipe.exit = { code: null, signal: null, stderr, error };
    done(pipe.exit);
    pipe.destroy(error);
  });
  child.once("close", (code, signal) => {
    pipe.exit = { code, signal, stderr };
    done(pipe.exit);
    pipe.destroy();
  });
  return pipe;
}

/** Connects the daemon client over a pipe; when that fails, waits briefly for
 * the child's exit so `classify({code, signal, stderr, error})` can name the
 * cause. `classify` returns an Error to throw. */
async function connectOverPipe({
  pipe,
  clientName = "desktop",
  helloTimeoutMs,
  classify,
}) {
  try {
    const client = await connectDaemon({
      clientName,
      helloTimeoutMs,
      connect: () => pipe,
    });
    client.exit = () => pipe.exit;
    return client;
  } catch (error) {
    pipe.destroy();
    // The daemon answered but cannot be used (e.g. another protocol): keep the
    // reason and never retry, the host needs an update.
    if (error.reason)
      throw failure(error.message, {
        state: "failed",
        reason: error.reason,
        retry: error.retry,
      });
    const exit = await Promise.race([
      pipe.exited,
      new Promise((resolve) => setTimeout(resolve, EXIT_GRACE_MS, null)),
    ]);
    throw await classify({ ...(exit || { code: null, stderr: "" }), error });
  }
}

function failure(message, extra = {}) {
  return Object.assign(new Error(message), extra);
}

const tail = (text) =>
  String(text || "")
    .trim()
    .split("\n")
    .slice(-3)
    .join(" ")
    .slice(0, 400);

/** Builds the remote connectors for a list of connection profiles, keyed by
 * the daemon host name (the profile id; the endpoint is `ssh:<id>`). */
function remoteConnectors(profiles, options) {
  const { createSshConnector } = require("./ssh.cjs");
  const { createCommandConnector } = require("./command.cjs");
  const out = {};
  for (const profile of profiles) {
    const connector =
      profile.connector?.kind === "command"
        ? createCommandConnector({ profile, ...options })
        : createSshConnector({ profile, ...options });
    // Only profiles with autoConnect connect on their own (see the manager).
    out[profile.id] = Object.assign(connector, {
      auto: Boolean(profile.autoConnect),
    });
  }
  return out;
}

module.exports = {
  spawnPipe,
  connectOverPipe,
  remoteConnectors,
  failure,
  tail,
  EXIT_GRACE_MS,
};
