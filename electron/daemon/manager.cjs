"use strict";

// Daemon manager: one entry per host, each with a connector that yields a
// connected, hello-checked client (client.cjs). The manager owns the state of
// every host, forwards daemon notifications, reconnects with backoff and
// re-attaches open terminal handles. Only host "local" exists in slice 1;
// slice 2 adds connectors (ssh, command) by adding entries to `connectors`.
//
// connector = { kind, connect(): Promise<client> }  (rejects with an Error that
//   may carry `reason`, e.g. "incompatible")
//
// manager API
//   start()                          connect every host; idempotent
//   close()                          drop connections and timers; never stops a daemon
//   states(): ConnectorState[]       {host, state, generation, reason?, message?, version?, capabilities?}
//   hello(host): {host, version, capabilities} | null   `host` is the daemon's own
//                                    host name (projects.sync must use it)
//   request(host, method, params)    rejects with RpcError, or Error when not ready
//   attach(host, id, {scrollback}, onBytes)
//        -> Promise<{cols, rows, seq, detach()}>
//   on("state", cb(ConnectorState)), on("event", cb(DaemonEvent)) -> unsubscribe
//        DaemonEvent = {host, generation, method, params}
//
// onBytes(data: Buffer, info). `info.snapshot === true` means `data` is the
// whole screen and REPLACES what the consumer shows: it is sent for the first
// attach, for a resync of a lagging subscriber, and after a reconnect, where
// `info.reattach === true` as well. Otherwise `data` is live output continuing
// from `info.seq`. `info.generation` is the connection generation it came from.
// A handle whose session the daemon no longer knows after a reconnect (RpcError)
// is dropped without a call; the session.exited / session.removed events tell
// the consumer. Any other error keeps the handle for the next reconnect.

const { EventEmitter } = require("node:events");

const BACKOFF_MIN_MS = 250;
const BACKOFF_MAX_MS = 30000;

function createDaemonManager({
  connectors,
  powerMonitor,
  backoffMinMs = BACKOFF_MIN_MS,
  backoffMaxMs = BACKOFF_MAX_MS,
  random = Math.random,
  log = () => {},
}) {
  const emitter = new EventEmitter();
  emitter.setMaxListeners(0);
  const hosts = new Map();
  let started = false;
  let closed = false;

  for (const [host, connector] of Object.entries(connectors))
    hosts.set(host, {
      host,
      connector,
      state: "offline",
      reason: undefined,
      message: undefined,
      generation: 0,
      client: null,
      hello: null,
      handles: new Set(),
      attempt: 0,
      timer: null,
      connecting: false,
    });

  function publicState(entry) {
    const out = {
      host: entry.host,
      state: entry.state,
      generation: entry.generation,
    };
    if (entry.reason) out.reason = entry.reason;
    if (entry.message) out.message = entry.message;
    if (entry.state === "ready" && entry.hello) {
      out.version = entry.hello.daemon;
      out.capabilities = entry.hello.capabilities;
    }
    return out;
  }

  function emit(name, payload) {
    try {
      emitter.emit(name, payload);
    } catch (error) {
      log(`${name} listener failed: ${error.message}`);
    }
  }

  function setState(entry, state, reason, message) {
    entry.state = state;
    entry.reason = reason;
    entry.message = message;
    emit("state", publicState(entry));
  }

  function entryFor(host) {
    const entry = hosts.get(host);
    if (!entry) throw new Error(`unknown host ${host}`);
    return entry;
  }

  function readyClient(host) {
    const entry = entryFor(host);
    if (entry.state !== "ready" || !entry.client)
      throw new Error(`host ${host} is not ready (${entry.state})`);
    return entry;
  }

  function schedule(entry) {
    if (closed || entry.timer) return;
    const base = Math.min(backoffMaxMs, backoffMinMs * 2 ** entry.attempt);
    entry.attempt += 1;
    const delay = Math.round(base * (0.75 + random() * 0.25));
    entry.timer = setTimeout(() => {
      entry.timer = null;
      void connect(entry);
    }, delay);
    entry.timer.unref?.();
  }

  // `again` marks the first snapshot after a reconnect with reattach: true.
  function wrap(handle, generation, again = false) {
    return (data, info) => {
      const flag = again && info.snapshot === true;
      if (info.snapshot) again = false;
      const out = { ...info, generation };
      if (flag) out.reattach = true;
      handle.onBytes(data, out);
    };
  }

  async function reattach(entry, client, generation) {
    await Promise.all(
      [...entry.handles].map(async (handle) => {
        try {
          handle.inner = await client.attach(
            handle.id,
            wrap(handle, generation, true),
          );
          if (handle.closed) await handle.inner.detach().catch(() => {});
        } catch (error) {
          if (error?.name === "RpcError") entry.handles.delete(handle);
          log(`re-attach ${handle.id} failed: ${error.message}`);
        }
      }),
    );
  }

  async function connect(entry) {
    if (closed || entry.connecting) return;
    entry.connecting = true;
    if (entry.timer) {
      clearTimeout(entry.timer);
      entry.timer = null;
    }
    setState(entry, "connecting");
    let client = null;
    try {
      client = await entry.connector.connect();
      // The list verifies the daemon serves requests before it is called ready;
      // consumers read the sessions themselves.
      await client.request("session.list", {});
    } catch (error) {
      client?.close();
      entry.connecting = false;
      log(`${entry.host}: connect failed: ${error.message}`);
      setState(entry, "failed", error.reason, error.message);
      schedule(entry);
      return;
    }
    entry.connecting = false;
    if (closed) {
      client.close();
      return;
    }
    entry.attempt = 0;
    entry.client = client;
    entry.hello = {
      host: client.hello.host || entry.host,
      daemon: client.hello.daemon,
      capabilities: client.hello.capabilities || [],
    };
    entry.generation += 1;
    const generation = entry.generation;
    client.on("notification", (method, params) => {
      if (entry.client !== client || method === "session.snapshot") return;
      emit("event", { host: entry.host, generation, method, params });
    });
    client.on("disconnect", () => {
      if (entry.client !== client) return;
      entry.client = null;
      if (closed) return;
      setState(
        entry,
        "offline",
        "daemon_died",
        "The connection to the daemon was lost.",
      );
      schedule(entry);
    });
    client.on("callback-error", (error) => log(`callback: ${error.message}`));
    client.on("protocol-error", (error) => log(`protocol: ${error.message}`));
    setState(entry, "ready");
    await reattach(entry, client, generation);
  }

  if (powerMonitor)
    powerMonitor.on("resume", () => {
      for (const entry of hosts.values())
        if (entry.timer || entry.state === "failed") {
          entry.attempt = 0;
          void connect(entry);
        }
    });

  return {
    start() {
      if (started) return;
      started = true;
      for (const entry of hosts.values()) void connect(entry);
    },
    close() {
      closed = true;
      for (const entry of hosts.values()) {
        clearTimeout(entry.timer);
        entry.timer = null;
        const client = entry.client;
        entry.client = null;
        client?.close();
      }
      emitter.removeAllListeners();
    },
    states: () => [...hosts.values()].map(publicState),
    hello(host) {
      const entry = entryFor(host);
      return entry.hello && entry.state === "ready"
        ? {
            host: entry.hello.host,
            version: entry.hello.daemon,
            capabilities: entry.hello.capabilities,
          }
        : null;
    },
    async request(host, method, params = {}) {
      return readyClient(host).client.request(method, params);
    },
    async attach(host, id, { scrollback } = {}, onBytes) {
      const entry = readyClient(host);
      const handle = {
        id,
        onBytes,
        inner: null,
        closed: false,
      };
      const generation = entry.generation;
      const inner = await entry.client.attach(id, wrap(handle, generation), {
        scrollback,
      });
      handle.inner = inner;
      entry.handles.add(handle);
      return {
        cols: inner.cols,
        rows: inner.rows,
        seq: inner.seq,
        async detach() {
          if (handle.closed) return;
          handle.closed = true;
          entry.handles.delete(handle);
          await handle.inner?.detach().catch(() => {});
        },
      };
    },
    on(name, callback) {
      emitter.on(name, callback);
      return () => emitter.off(name, callback);
    },
  };
}

module.exports = { createDaemonManager };
