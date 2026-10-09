"use strict";

// Daemon client: hello handshake, JSON-RPC request matching with timeouts,
// notifications as events, and ordered terminal output via attach().
// Reconnecting is the caller's decision; a lost connection emits "disconnect"
// and rejects every pending request.

const net = require("node:net");
const { EventEmitter } = require("node:events");
const { createDecoder, encode } = require("./frame.cjs");

const PROTOCOL_VERSION = 1;
const DEFAULT_TIMEOUT_MS = 10000;
const MAX_WRITE_BACKLOG = 8 * 1024 * 1024;

class RpcError extends Error {
  constructor(error) {
    super(error.message);
    this.name = "RpcError";
    this.code = error.code;
  }
}

function connectDaemon({
  socketPath,
  clientName,
  connect = net.createConnection,
  helloTimeoutMs = DEFAULT_TIMEOUT_MS,
}) {
  const events = new EventEmitter();
  const decoder = createDecoder();
  const pending = new Map();
  const attached = new Map();
  // Requests the peer sends us (a companion process asking the app), by method.
  const handlers = new Map();
  let nextId = 1;
  let closed = false;
  let socket;

  function request(
    method,
    params = {},
    { timeoutMs = DEFAULT_TIMEOUT_MS } = {},
  ) {
    if (closed) return Promise.reject(new Error("daemon connection closed"));
    if (socket.writableLength > MAX_WRITE_BACKLOG)
      return Promise.reject(
        new Error(`${method}: backpressure, daemon is not reading`),
      );
    const id = nextId++;
    let bytes;
    try {
      bytes = encode({
        kind: "J",
        json: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      });
    } catch (error) {
      return Promise.reject(error);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      pending.set(id, { resolve, reject, timer });
      socket.write(bytes);
    });
  }

  function failPending(error) {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(error);
    }
    pending.clear();
  }

  // ponytail: JSON seq is a Number; exact up to 2^53 bytes (~9 PB) of output per session
  function toBigInt(seq) {
    return BigInt(seq);
  }

  function report(name, error) {
    try {
      events.emit(name, error);
    } catch {
      // A throwing error listener must not break the stream either.
    }
  }

  function emitBytes(state, data, info) {
    try {
      state.onBytes(data, info);
    } catch (error) {
      report("callback-error", error);
    }
  }

  // Applies a snapshot and the output buffered while it was in flight.
  function applySnapshot(id, state, snap) {
    if (attached.get(id) !== state) return;
    state.next = toBigInt(snap.seq);
    state.syncing = false;
    emitBytes(state, Buffer.from(snap.snapshot, "base64"), {
      snapshot: true,
      seq: state.next,
      cols: snap.cols,
      rows: snap.rows,
    });
    const queued = state.queue;
    state.queue = [];
    for (const frame of queued) deliver(id, state, frame);
  }

  // The daemon guarantees gapless output and resyncs a lagging subscriber with
  // session.snapshot. A gap is reported once per attach and delivery goes on.
  function deliver(id, state, frame) {
    if (attached.get(id) !== state) return;
    if (state.syncing) {
      state.queue.push(frame);
      return;
    }
    const end = frame.seq + BigInt(frame.data.length);
    if (end <= state.next) return; // stale: already covered by the snapshot
    if (frame.seq > state.next && !state.gapReported) {
      state.gapReported = true;
      report(
        "protocol-error",
        new Error(
          `output gap for ${id}: expected ${state.next}, got ${frame.seq}`,
        ),
      );
    }
    const data =
      frame.seq < state.next
        ? frame.data.subarray(Number(state.next - frame.seq))
        : frame.data;
    state.next = end;
    emitBytes(state, data, {
      snapshot: false,
      seq: frame.seq + BigInt(frame.data.length - data.length),
    });
  }

  function onFrame(frame) {
    if (frame.kind === "B") {
      const state = attached.get(frame.id);
      if (state) deliver(frame.id, state, frame);
      return;
    }
    let message;
    try {
      message = JSON.parse(frame.json);
    } catch {
      return;
    }
    if (message.method && message.id !== undefined) {
      void answer(message);
      return;
    }
    if (message.method && message.id === undefined) {
      const state =
        message.method === "session.snapshot" &&
        message.params &&
        attached.get(message.params.id);
      if (state) applySnapshot(message.params.id, state, message.params);
      events.emit(message.method, message.params);
      events.emit("notification", message.method, message.params);
      return;
    }
    const entry = pending.get(message.id);
    if (!entry) return;
    pending.delete(message.id);
    clearTimeout(entry.timer);
    if (message.error) entry.reject(new RpcError(message.error));
    else entry.resolve(message.result);
  }

  function reply(body) {
    if (closed || socket.destroyed) return;
    socket.write(
      encode({ kind: "J", json: JSON.stringify({ jsonrpc: "2.0", ...body }) }),
    );
  }

  // A request from the peer: run its handler and send back the result or a
  // JSON-RPC error. A handler error may carry a numeric `code`.
  async function answer({ id, method, params }) {
    const handler = handlers.get(method);
    if (!handler)
      return reply({ id, error: { code: -32601, message: "unknown method" } });
    try {
      reply({ id, result: (await handler(params ?? {})) ?? null });
    } catch (error) {
      reply({
        id,
        error: {
          code: Number.isInteger(error?.code) ? error.code : -32000,
          message: String(error?.message || error),
        },
      });
    }
  }

  function onClose(error) {
    if (closed) return;
    closed = true;
    failPending(error || new Error("daemon connection lost"));
    events.emit("disconnect", error);
  }

  // onBytes(data, info): `info.snapshot === true` means `data` is the whole
  // screen (first attach, resync or a lagging subscriber) and replaces what the
  // consumer shows; otherwise `data` is live output continuing from `info.seq`.
  // `scrollback` asks the daemon to prepend that many history lines to the
  // first snapshot.
  async function attach(id, onBytes, { scrollback } = {}) {
    if (attached.has(id)) throw new Error(`session ${id} is already attached`);
    const state = {
      onBytes,
      next: 0n,
      syncing: true,
      queue: [],
      gapReported: false,
    };
    attached.set(id, state);
    try {
      const snap = await request("session.attach", { id, scrollback });
      applySnapshot(id, state, snap);
      return {
        cols: snap.cols,
        rows: snap.rows,
        seq: toBigInt(snap.seq),
        detach() {
          if (attached.get(id) === state) attached.delete(id);
          return request("session.detach", { id });
        },
      };
    } catch (error) {
      if (attached.get(id) === state) attached.delete(id);
      throw error;
    }
  }

  function close() {
    if (closed) return;
    closed = true;
    failPending(new Error("daemon connection closed"));
    socket.destroy();
  }

  const api = Object.assign(events, {
    request,
    /** Sends a notification (no reply expected). */
    notify(method, params = {}) {
      reply({ method, params });
    },
    /** Answers the peer's requests of this method with `handler(params)`. */
    handle(method, handler) {
      handlers.set(method, handler);
    },
    attach,
    close,
    pendingCount: () => pending.size,
  });

  return new Promise((resolve, reject) => {
    socket = connect(socketPath);
    socket.on("data", (chunk) => {
      let frames;
      try {
        frames = decoder.push(chunk);
      } catch (error) {
        socket.destroy();
        onClose(error);
        return;
      }
      for (const frame of frames) {
        try {
          onFrame(frame);
        } catch (error) {
          report("callback-error", error);
        }
      }
    });
    socket.on("error", (error) => {
      reject(error);
      onClose(error);
    });
    socket.on("close", () => {
      reject(new Error("daemon connection closed before hello"));
      onClose();
    });
    const start = () => {
      request(
        "hello",
        { protocol: PROTOCOL_VERSION, client: clientName },
        { timeoutMs: helloTimeoutMs },
      ).then(
        (result) => {
          if (result.protocol !== PROTOCOL_VERSION) {
            close();
            reject(
              Object.assign(
                new Error(
                  `daemon speaks protocol ${result.protocol}, expected ${PROTOCOL_VERSION}`,
                ),
                { reason: "incompatible", retry: false },
              ),
            );
            return;
          }
          api.hello = result;
          resolve(api);
        },
        (error) => {
          close();
          reject(error);
        },
      );
    };
    if (socket.connecting === false) start();
    else socket.once("connect", start);
  });
}

module.exports = { connectDaemon, RpcError, PROTOCOL_VERSION };
