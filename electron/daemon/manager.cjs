"use strict";

// Daemon manager: one entry per host, each with a connector that yields a
// connected, hello-checked client (client.cjs). The manager owns the state of
// every host, forwards daemon notifications, reconnects with backoff and
// re-attaches open terminal handles. Hosts: "local" plus one per connection
// profile (ssh or command connector, see connectors.cjs); `setConnectors` adds,
// replaces and removes hosts while the manager runs.
//
// connector = { kind, signature?, ping?, connect(), describeLoss?(client) }
//   connect() resolves a client or rejects with an Error that may carry
//     state ("failed" default | "need_auth"), hint (a command the user can
//     copy to fix it), reason ("incompatible",
//     "host_key_changed", "not_installed", ...), retry === false (no automatic
//     retry: the user has to act) and retryNow (reconnect at once, once).
//   describeLoss(client) -> {state, reason, message, retry?, retryNow?} | null
//     says why a connected client went away.
//   ping = {intervalMs, timeoutMs}: the manager sends `$/ping` and drops the
//     connection when it is not answered in time.
//   signature: setConnectors replaces a host's connector when it changes.
//
// manager API
//   start()                          connect every host; idempotent
//   close()                          drop connections and timers; never stops a daemon
//   setConnectors({host: connector}) add, replace (changed signature) and remove
//                                    hosts; a removed host ends with state
//                                    offline, reason "removed"; a host whose
//                                    connector is unchanged but stopped without
//                                    a retry (need_auth, ...) is retried
//   retry(host): Promise<ConnectorState>  reconnect now (one attempt)
//   states(): ConnectorState[]       {host, state, generation, reason?, message?, version?, capabilities?, update?}
//   hello(host): {host, version, capabilities, build?} | null   `host` is the daemon's own
//                                    host name (projects.sync must use it);
//                                    `build` is the sha256 of its binary
//   setUpdate(host, bool)            flags a ready host whose binary differs
//                                    from the bundled one (`update: true`)
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
const PING_METHOD = "$/ping";
const REATTACH_SCROLLBACK = 2000;
// A connection counts as healthy, and the retry budget is refilled, only after
// it has stayed up this long: a daemon that dies right after "ready" must not
// earn a fresh immediate reconnect every time.
const STABLE_MS = 10000;

function createDaemonManager({
  connectors,
  powerMonitor,
  backoffMinMs = BACKOFF_MIN_MS,
  backoffMaxMs = BACKOFF_MAX_MS,
  stableMs = STABLE_MS,
  random = Math.random,
  log = () => {},
}) {
  const emitter = new EventEmitter();
  emitter.setMaxListeners(0);
  const hosts = new Map();
  let started = false;
  let closed = false;

  function addHost(host, connector) {
    const entry = {
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
      immediateUsed: false,
      timer: null,
      pingTimer: null,
      stableTimer: null,
      auto: connector.auto,
      explicit: false, // the user connected it by hand
      suspended: false, // the user disconnected it
      pending: null,
      token: 0, // bumped when a running attempt must not take effect
      removed: false,
    };
    hosts.set(host, entry);
    return entry;
  }
  for (const [host, connector] of Object.entries(connectors))
    addHost(host, connector);

  function publicState(entry) {
    const out = {
      host: entry.host,
      state: entry.state,
      generation: entry.generation,
    };
    if (entry.reason) out.reason = entry.reason;
    if (entry.message) out.message = entry.message;
    if (entry.hint) out.hint = entry.hint;
    if (entry.state === "ready" && entry.hello) {
      out.version = entry.hello.daemon;
      out.capabilities = entry.hello.capabilities;
      if (entry.update) out.update = true;
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

  function setState(entry, state, reason, message, hint) {
    entry.hint = hint;
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

  // `now` asks for an immediate reconnect, granted once until a connection
  // succeeds, so a daemon that dies at once cannot spin.
  function schedule(entry, now = false) {
    if (closed || entry.removed || entry.suspended || entry.timer) return;
    let delay = 0;
    if (now && !entry.immediateUsed) entry.immediateUsed = true;
    else {
      const base = Math.min(backoffMaxMs, backoffMinMs * 2 ** entry.attempt);
      entry.attempt += 1;
      delay = Math.round(base * (0.75 + random() * 0.25));
    }
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

  // Every reattach asks for the history again: the consumer's screen was reset
  // by the snapshot that comes back. A failure that is not the daemon refusing
  // (RpcError) is tried again with backoff while this connection lasts.
  async function reattachOne(entry, client, generation, handle, tries = 0) {
    if (handle.closed || entry.client !== client) return;
    handle.inner = null;
    try {
      handle.inner = await client.attach(
        handle.id,
        wrap(handle, generation, true),
        { scrollback: REATTACH_SCROLLBACK },
      );
      if (handle.closed) await handle.inner.detach().catch(() => {});
    } catch (error) {
      log(`re-attach ${handle.id} failed: ${error.message}`);
      if (error?.name === "RpcError") {
        entry.handles.delete(handle);
        return;
      }
      const delay = Math.min(backoffMaxMs, backoffMinMs * 2 ** tries);
      const timer = setTimeout(
        () => void reattachOne(entry, client, generation, handle, tries + 1),
        delay,
      );
      timer.unref?.();
    }
  }

  async function reattach(entry, client, generation) {
    await Promise.all(
      [...entry.handles].map((handle) =>
        reattachOne(entry, client, generation, handle),
      ),
    );
  }

  function stopPing(entry) {
    clearInterval(entry.pingTimer);
    entry.pingTimer = null;
    clearTimeout(entry.stableTimer);
    entry.stableTimer = null;
  }

  // Hosts of profiles without autoConnect wait until the user connects them.
  const wants = (entry) =>
    !entry.suspended && (entry.auto !== false || entry.explicit);

  function startPing(entry, client) {
    const ping = entry.connector.ping;
    if (!ping) return;
    entry.pingTimer = setInterval(async () => {
      try {
        await client.request(PING_METHOD, {}, { timeoutMs: ping.timeoutMs });
      } catch (error) {
        if (entry.client !== client) return;
        log(`${entry.host}: ping failed: ${error.message}`);
        lost(entry, client, {
          state: "offline",
          reason: "ping_timeout",
          message: "The daemon stopped answering.",
        });
      }
    }, ping.intervalMs);
    entry.pingTimer.unref?.();
  }

  // The connected client is gone: drop it and decide how to reconnect.
  async function lost(entry, client, description) {
    if (entry.client !== client) return;
    entry.client = null;
    stopPing(entry);
    client.close();
    if (closed || entry.removed || entry.suspended) return;
    const token = entry.token;
    const why = description ||
      (await entry.connector.describeLoss?.(client)) || {
        state: "offline",
        reason: "daemon_died",
        message: "The connection to the daemon was lost.",
      };
    if (
      closed ||
      entry.removed ||
      entry.suspended ||
      entry.token !== token ||
      entry.client
    )
      return;
    setState(entry, why.state || "offline", why.reason, why.message, why.hint);
    if (why.retry !== false) schedule(entry, why.retryNow);
  }

  function connect(entry) {
    if (closed || entry.removed) return Promise.resolve();
    if (entry.pending) return entry.pending;
    if (entry.timer) {
      clearTimeout(entry.timer);
      entry.timer = null;
    }
    const pending = attempt(entry).finally(() => {
      if (entry.pending === pending) entry.pending = null;
    });
    entry.pending = pending;
    return pending;
  }

  async function attempt(entry) {
    setState(entry, "connecting");
    const connector = entry.connector;
    const token = entry.token;
    const stale = () =>
      closed ||
      entry.removed ||
      entry.suspended ||
      entry.token !== token ||
      entry.connector !== connector;
    let client = null;
    try {
      client = await connector.connect();
      // The list verifies the daemon serves requests before it is called ready;
      // consumers read the sessions themselves.
      await client.request("session.list", {});
    } catch (error) {
      client?.close();
      if (stale()) return;
      log(`${entry.host}: connect failed: ${error.message}`);
      setState(
        entry,
        error.state || "failed",
        error.reason,
        error.message,
        error.hint,
      );
      if (error.retry !== false) schedule(entry, error.retryNow);
      return;
    }
    if (stale()) {
      client.close();
      return;
    }
    entry.client = client;
    clearTimeout(entry.stableTimer);
    entry.stableTimer = setTimeout(() => {
      entry.attempt = 0;
      entry.immediateUsed = false;
    }, stableMs);
    entry.stableTimer.unref?.();
    entry.hello = {
      host: client.hello.host || entry.host,
      daemon: client.hello.daemon,
      capabilities: client.hello.capabilities || [],
      build: client.hello.build,
    };
    entry.update = false;
    entry.generation += 1;
    const generation = entry.generation;
    client.on("notification", (method, params) => {
      if (entry.client !== client || method === "session.snapshot") return;
      emit("event", { host: entry.host, generation, method, params });
    });
    client.on("disconnect", () => lost(entry, client));
    client.on("callback-error", (error) => log(`callback: ${error.message}`));
    client.on("protocol-error", (error) => log(`protocol: ${error.message}`));
    setState(entry, "ready");
    startPing(entry, client);
    await reattach(entry, client, generation);
  }

  function dropHost(entry) {
    entry.token += 1;
    clearTimeout(entry.timer);
    entry.timer = null;
    stopPing(entry);
    const client = entry.client;
    entry.client = null;
    client?.close();
  }

  // Terminal states that stop without a retry, which a new save may retry.
  const stopped = (entry) =>
    !entry.timer && !entry.pending && entry.state !== "ready";

  if (powerMonitor)
    powerMonitor.on("resume", () => {
      for (const entry of hosts.values())
        if (wants(entry) && (entry.timer || stopped(entry))) {
          entry.attempt = 0;
          entry.immediateUsed = false;
          void connect(entry);
        }
    });

  return {
    start() {
      if (started) return;
      started = true;
      for (const entry of hosts.values()) if (wants(entry)) void connect(entry);
    },
    close() {
      closed = true;
      for (const entry of hosts.values()) dropHost(entry);
      emitter.removeAllListeners();
    },
    setConnectors(next) {
      if (closed) return;
      for (const [host, entry] of [...hosts]) {
        if (Object.hasOwn(next, host)) continue;
        entry.removed = true;
        dropHost(entry);
        hosts.delete(host);
        setState(entry, "offline", "removed", "This host was removed.");
      }
      for (const [host, connector] of Object.entries(next)) {
        const entry = hosts.get(host);
        if (!entry) {
          const added = addHost(host, connector);
          if (started && wants(added)) void connect(added);
          continue;
        }
        entry.auto = connector.auto;
        const changed =
          connector.signature === undefined ||
          connector.signature !== entry.connector.signature;
        if (changed && entry.connector !== connector) {
          dropHost(entry);
          entry.pending = null;
          entry.connector = connector;
          entry.attempt = 0;
          entry.immediateUsed = false;
          if (started && wants(entry)) void connect(entry);
        } else if (started && wants(entry) && stopped(entry)) {
          entry.attempt = 0;
          entry.immediateUsed = false;
          void connect(entry);
        }
      }
    },
    // The user connects a host by hand (Connect, Retry, Install).
    async retry(host) {
      const entry = entryFor(host);
      entry.explicit = true;
      entry.suspended = false;
      entry.token += 1;
      entry.pending = null;
      if (entry.state === "ready" && entry.client) {
        const client = entry.client;
        entry.client = null;
        stopPing(entry);
        client.close();
      }
      entry.attempt = 0;
      entry.immediateUsed = false;
      await connect(entry);
      return publicState(entry);
    },
    // The user disconnects a host: it stays listed, offline, and nothing
    // reconnects it until the next retry().
    disconnect(host) {
      const entry = entryFor(host);
      entry.suspended = true;
      entry.explicit = false;
      dropHost(entry);
      entry.pending = null;
      setState(entry, "offline", "disconnected", "Disconnected.");
    },
    states: () => [...hosts.values()].map(publicState),
    hello(host) {
      const entry = entryFor(host);
      return entry.hello && entry.state === "ready"
        ? {
            host: entry.hello.host,
            version: entry.hello.daemon,
            capabilities: entry.hello.capabilities,
            ...(entry.hello.build ? { build: entry.hello.build } : {}),
          }
        : null;
    },
    // The bundled sushiai differs from the daemon's build: the row offers an
    // update. Cleared by the next connection.
    setUpdate(host, update) {
      const entry = hosts.get(host);
      if (!entry || entry.state !== "ready" || entry.update === update) return;
      entry.update = update;
      emit("state", publicState(entry));
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
