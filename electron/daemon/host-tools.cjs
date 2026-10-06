"use strict";

// Tool setup (Claude Code, Codex, skills) for a daemon host that reaches
// ready for the first time. It runs once per host: the result is recorded, so
// a reconnect never reruns the installers. Command hosts have no shell and are
// skipped with a note in the log. A ready host whose daemon binary differs from
// the bundled one (hello.build against the manifest's sha256 for the host's
// platform) is flagged for update.

function watchHostTools({
  manager,
  connections,
  store,
  setup,
  manifest = () => null,
  log = () => {},
}) {
  const running = new Set();
  const platforms = new Map();
  const checked = new Set(); // host:generation already compared

  // The bundled sushiai for the host's platform, or null when unknown.
  async function bundledSha(endpoint, host) {
    let platform = platforms.get(host);
    if (!platform) {
      platform = (
        await connections.exec(endpoint, "sh -c 'uname -sm'", {
          timeout: 30000,
        })
      ).trim();
      platforms.set(host, platform);
    }
    return manifest()?.manifest?.[platform]?.sha256 || null;
  }

  async function checkBuild(host) {
    const endpoint = `ssh:${host}`;
    try {
      if (!connections.hasShell(endpoint)) return;
      const sha = await bundledSha(endpoint, host);
      const hello = manager.hello(host);
      // An older daemon reports no build: it is older than the bundled one.
      if (sha && hello) manager.setUpdate(host, hello.build !== sha);
    } catch (error) {
      log(`${host}: build check failed: ${error.message}`);
    }
  }

  const done = () => store.read();

  async function run(host) {
    const endpoint = `ssh:${host}`;
    let shell;
    try {
      shell = connections.hasShell(endpoint);
    } catch {
      return; // the profile is gone
    }
    if (!shell) {
      log(`${host}: connects through a command, tool setup skipped`);
      return;
    }
    running.add(host);
    try {
      // sushiai is already there: the daemon answered.
      await setup(endpoint, { sushiai: null });
      store.write({ ...done(), [host]: true });
    } catch (error) {
      log(`${host}: tool setup failed: ${error.message}`);
    } finally {
      running.delete(host);
    }
  }

  const off = manager.on("state", (state) => {
    if (state.state !== "ready" || state.host === "local") return;
    if (!state.update && !checked.has(`${state.host}:${state.generation}`)) {
      checked.add(`${state.host}:${state.generation}`);
      void checkBuild(state.host);
    }
    if (running.has(state.host) || done()[state.host]) return;
    void run(state.host);
  });
  return {
    off,
    mark: (host) => store.write({ ...done(), [host]: true }),
  };
}

module.exports = { watchHostTools };
