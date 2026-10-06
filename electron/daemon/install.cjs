"use strict";

// The `host-install` handler: sets a remote host up through the one installer
// (host-setup.cjs setupHost: Claude Code and Codex tools, skill sync, then the
// bundled sushiai), restarts the daemon there and waits for the manager to
// report it ready.
//
//   1. setupHost with the host manifest (target/host in dev, <resources>/host
//      when packaged) and no Herdr steps.
//   2. The old daemon stops: `daemon.shutdown` when one is connected (an old
//      daemon answered through a new proxy drops its answers at EOF, so the
//      next proxy has to start the new one; sessions live in their holders and
//      survive). A daemon the app cannot talk to (another protocol) is stopped
//      over exec instead, by pid, and only after the lock proves it is held.
//   3. manager.retry(host) until ready, or a state that needs the owner.

const READY_ATTEMPTS = 6;
const SETTLE_MS = 400;
// Another attempt cannot help: the owner has to act.
const needsOwner = (state) =>
  state.state === "need_auth" || state.reason === "host_key_changed";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const quote = (text) => `'${String(text).replaceAll("'", "'\\''")}'`;

// Stops the daemon of ~/.sushiai whatever protocol it speaks. The pid in
// daemon.lock is trusted only when the lock is held: taking the flock fails
// then. After a crash the file keeps a stale pid, which must never be signalled.
const STOP_DAEMON_PY = `
import fcntl, os, signal, sys
path = os.path.join(os.path.expanduser("~"), ".sushiai", "daemon.lock")
try:
    handle = open(path, "r+")
except OSError:
    print("nolock")
    sys.exit(0)
try:
    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    print("free")
    sys.exit(0)
except OSError:
    pass
try:
    pid = int(handle.read().strip())
except ValueError:
    print("nopid")
    sys.exit(0)
if pid > 1:
    os.kill(pid, signal.SIGTERM)
    print("stopped")
`;
const STOP_DAEMON_COMMAND = `sh -c ${quote(
  `export PATH="$HOME/.local/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"; python3 -c ${quote(STOP_DAEMON_PY)}`,
)}`;

function createHostInstaller({
  manager,
  connections,
  manifest,
  setup,
  markSetup = () => {},
  settleMs = SETTLE_MS,
  attempts = READY_ATTEMPTS,
}) {
  async function installOn(host) {
    if (host === "local")
      throw new Error("This Mac runs the daemon that ships with the app.");
    const endpoint = `ssh:${host}`;
    // Throws "unknown host" before anything runs.
    if (!connections.hasShell(endpoint))
      throw new Error(
        "This host connects through a command. Install sushiai there yourself.",
      );
    const { manifest: entries, binDir } = manifest();
    const states = await setup(endpoint, {
      sushiai: { manifest: entries, binDir },
    });
    if (states.sushiai === "failed")
      throw new Error(states.sushiaiError || "Installing sushiai failed.");
    const result = states.sushiaiResult;
    if (!result) throw new Error("The installer did not report sushiai.");
    markSetup(host);
    const before = manager.states().find((state) => state.host === host);
    if (before?.state === "ready") {
      await manager.request(host, "daemon.shutdown", {}).catch(() => {});
      await sleep(settleMs);
    } else {
      await connections
        .exec(endpoint, STOP_DAEMON_COMMAND, { timeout: 30000 })
        .catch(() => {});
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

module.exports = { createHostInstaller, STOP_DAEMON_COMMAND };
