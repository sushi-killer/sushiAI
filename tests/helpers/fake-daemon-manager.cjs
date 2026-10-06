// An in-process stand-in for the daemon manager (electron/daemon/manager.cjs)
// as the orchestrator sees it: host states, `on("state" | "event")`,
// `request(host, method, params, options)` and `retry(host)`.
//
//   hosts     { name: { state?, capabilities?, message? } }, default local ready
//             with the `orch` capability
//   handlers  { "orch.task.list": (params, host) => result }; a missing handler
//             answers {}; a handler may throw (code is kept on the error)
const { EventEmitter } = require("node:events");

function fakeDaemonManager({ hosts, handlers = {}, onRetry } = {}) {
  const emitter = new EventEmitter();
  const table = new Map(
    Object.entries(
      hosts ?? { local: { state: "ready", capabilities: ["orch"] } },
    ).map(([host, state]) => [
      host,
      { host, state: "ready", capabilities: ["orch"], ...state },
    ]),
  );
  const calls = [];
  const publicState = (entry) => ({
    ...entry,
    ...(entry.state === "ready" ? {} : { capabilities: undefined }),
  });
  const manager = {
    calls,
    handlers,
    states: () => [...table.values()].map(publicState),
    on(name, callback) {
      emitter.on(name, callback);
      return () => emitter.off(name, callback);
    },
    async request(host, method, params = {}, options) {
      const entry = table.get(host);
      if (!entry || entry.state !== "ready")
        throw new Error(`host ${host} is not ready (${entry?.state})`);
      calls.push({ host, method, params, options });
      return (await handlers[method]?.(params, host)) ?? {};
    },
    async retry(host) {
      onRetry?.(host, manager);
      return publicState(table.get(host));
    },
    /** Changes a host's state and announces it, like the real manager. */
    setState(host, patch) {
      const entry = table.get(host) ?? { host, capabilities: ["orch"] };
      table.set(host, { ...entry, ...patch });
      emitter.emit("state", publicState(table.get(host)));
    },
    /** A daemon notification of a host. */
    notify(host, method, params) {
      emitter.emit("event", { host, generation: 1, method, params });
    },
    listeners: (name) => emitter.listenerCount(name),
    requestsFor: (method) => calls.filter((call) => call.method === method),
  };
  return manager;
}

module.exports = { fakeDaemonManager };
