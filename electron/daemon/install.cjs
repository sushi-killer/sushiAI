"use strict";

// The `host-install` handler: installs the bundled sushiai on a remote host,
// restarts the daemon there and waits for the manager to report it ready.
//
//   1. installSushiai over connections.exec (host-install.cjs), with the host
//      manifest (target/host in dev, <resources>/host when packaged).
//   2. `daemon.shutdown` when a daemon is connected: an old daemon answered
//      through a new proxy drops its answers at EOF, so the next proxy has to
//      start the new one. Sessions live in their holders and survive.
//   3. manager.retry(host) until ready, or a state that needs the owner.

const { installSushiai } = require("../host-install.cjs");

const READY_ATTEMPTS = 6;
const SETTLE_MS = 400;
// Another attempt cannot help: the owner has to act.
const needsOwner = (state) =>
  state.state === "need_auth" || state.reason === "host_key_changed";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function createHostInstaller({
  manager,
  connections,
  manifest,
  install = installSushiai,
  settleMs = SETTLE_MS,
  attempts = READY_ATTEMPTS,
}) {
  async function installOn(host) {
    if (host === "local")
      throw new Error("This Mac runs the daemon that ships with the app.");
    const endpoint = `ssh:${host}`;
    // Throws "unknown host" / "no shell access" before anything runs.
    if (!connections.hasShell(endpoint))
      throw new Error(
        "This host connects through a command. Install sushiai there yourself.",
      );
    const { manifest: entries, binDir } = manifest();
    const result = await install({
      exec: (command, options) => connections.exec(endpoint, command, options),
      manifest: entries,
      binDir,
    });
    const before = manager.states().find((state) => state.host === host);
    if (before?.state === "ready") {
      await manager.request(host, "daemon.shutdown", {}).catch(() => {});
      await sleep(settleMs);
    }
    let state = before;
    for (let i = 0; i < attempts; i++) {
      state = await manager.retry(host);
      if (state.state === "ready") return result;
      if (needsOwner(state)) break;
      await sleep(settleMs);
    }
    throw new Error(
      `sushiai was installed, but the daemon on the host is not ready: ${state?.message || state?.state || "unknown"}`,
    );
  }
  return { install: installOn };
}

module.exports = { createHostInstaller };
