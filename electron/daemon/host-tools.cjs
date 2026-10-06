"use strict";

// Tool setup (Claude Code, Codex, skills) for a daemon host that reaches
// ready for the first time. It runs once per host: the result is recorded, so
// a reconnect never reruns the installers. Command hosts have no shell and are
// skipped with a note in the log.

// One setup per host at a time: the first-ready tool setup and a manual
// install share the host's in-flight promise, so a second call starts only
// when the first has settled (it keeps its own options).
function serializePerHost(setup) {
  const inFlight = new Map();
  return (endpoint, options) => {
    const previous = inFlight.get(endpoint) || Promise.resolve();
    const run = previous.catch(() => {}).then(() => setup(endpoint, options));
    const tail = run.catch(() => {});
    inFlight.set(endpoint, tail);
    void tail.then(() => {
      if (inFlight.get(endpoint) === tail) inFlight.delete(endpoint);
    });
    return run;
  };
}

function watchHostTools({
  manager,
  connections,
  store,
  setup,
  log = () => {},
}) {
  const running = new Set();
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
    if (running.has(state.host) || done()[state.host]) return;
    void run(state.host);
  });
  return {
    off,
    mark: (host) => store.write({ ...done(), [host]: true }),
  };
}

module.exports = { watchHostTools, serializePerHost };
