"use strict";

const { spawn: nodeSpawn } = require("node:child_process");
const { randomBytes } = require("node:crypto");
const { homedir } = require("node:os");
const { join } = require("node:path");

const LIMITS = Object.freeze({
  startupMs: 90_000,
  requestMs: 30_000,
  connectMs: 10_000,
  messageBytes: 2 * 1024 * 1024,
  lineBytes: 8192,
  urlBytes: 8192,
  pending: 128,
  killMs: 2000,
  reconnectAttempts: 5,
  reconnectBaseMs: 500,
  reconnectMaxMs: 8000,
  stableMs: 60_000,
  heartbeatMs: 15_000,
  heartbeatTimeoutMs: 10_000,
  replayMs: 10_000,
  sessions: 64,
  replayEvents: 512,
  holdEvents: 2048,
  holdBytes: 8 * 1024 * 1024,
  errorBytes: 8192,
});
const transportErrors = new WeakSet();
const failure = (code) => {
  const err = Object.assign(new Error(`Hermes transport: ${code}`), { code });
  transportErrors.add(err);
  return err;
};
const record = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);
function profileName(value) {
  if (value == null || value === "") return "current";
  if (
    typeof value !== "string" ||
    value.length > 128 ||
    /[\x00-\x1f\x7f/\\]/.test(value)
  )
    throw failure("INVALID_PROFILE");
  return value.trim() || "current";
}
function encode(value) {
  let json;
  try {
    json = JSON.stringify(value);
  } catch {
    throw failure("INVALID_JSON");
  }
  if (typeof json !== "string") throw failure("INVALID_JSON");
  if (Buffer.byteLength(json) > LIMITS.messageBytes)
    throw failure("MESSAGE_TOO_LARGE");
  return json;
}

/** Main-process only. Never pass this instance or injected factories to IPC.
 * constructor({executable=~/.local/bin/hermes, profile='default', env={},
 *              onEvent, onState, spawn?, fetcher?, socketFactory?})
 * start(): Promise<void>; request()/rpc(): Promise<JSON value>; close(): Promise<void>.
 * request(method, '/api/...', {profile=this profile, body?, query?}={})
 * rpc(profile=this profile, method, params={}): rejects other profiles.
 * Named profiles launch `hermes --profile NAME serve ...`; default uses the
 * launch environment (including an explicitly isolated HERMES_HOME).
 * env overlays process.env; the owned token and parent PID always win.
 * HERMES_DESKTOP is omitted unless explicitly supplied in env.
 * onEvent receives native event params plus an authoritative `profile` tag.
 * onState receives {state, profile?, code?}. Callback exceptions are isolated.
 * Injected fetcher follows Fetch; socketFactory(url) returns a WHATWG WebSocket.
 * Once RPC demand opens a socket, reconnect is automatic (five backed-off
 * attempts, reset after 60s stable or new demand after exhaustion). No user
 * RPC is resent. Heartbeat runs only when gateway.ready advertises support.
 * Recovery holds live frames and replays up to 64 session sequence watermarks.
 * transport.resync has {session_id, payload:{reason}}; parent refetches history.
 * Errors keep transport `code`, native `rpcCode`/`status`, bounded redacted
 * `detail` and `data`, plus an actionable message. Child diagnostics stay private.
 */
class HermesTransport {
  #executable;
  #spawn;
  #fetch;
  #socketFactory;
  #onEvent;
  #onState;
  #profile;
  #env;
  #token;
  #child;
  #port;
  #start;
  #startupReject;
  #startupTimer;
  #terminal;
  #sockets = new Map();
  #operations = new Set();
  #nextId = 0;
  #reconnectTimer;
  #attempts = 0;
  #everOpened = false;
  #epoch;
  #sequences = new Map();

  constructor({
    executable = join(homedir(), ".local/bin/hermes"),
    profile = "default",
    env = {},
    onEvent,
    onState,
    spawn = nodeSpawn,
    fetcher = globalThis.fetch,
    socketFactory = (url) => new globalThis.WebSocket(url),
  } = {}) {
    this.#executable = executable;
    this.#spawn = spawn;
    this.#fetch = fetcher;
    this.#socketFactory = socketFactory;
    this.#onEvent = onEvent;
    this.#onState = onState;
    this.#profile = profileName(profile);
    if (this.#profile === "current" || this.#profile.startsWith("-"))
      throw failure("INVALID_PROFILE");
    this.#env = { ...env };
  }

  #notify(callback, value) {
    try {
      callback?.(value);
    } catch {
      /* Consumer owns callback failures. */
    }
  }
  #state(state, extra = {}) {
    this.#notify(this.#onState, { state, ...extra });
  }
  #check() {
    if (this.#terminal) throw failure(this.#terminal);
  }
  #nativeError(code, native, status) {
    const err = failure(code);
    if (status) err.status = status;
    let budget = LIMITS.errorBytes,
      nodes = 256;
    const clean = (value, depth = 0) => {
      if (--nodes < 0 || depth > 6 || budget <= 0) return "[truncated]";
      if (typeof value === "string") {
        const safe = value
          .split(this.#token)
          .join("[redacted]")
          .replace(/[\x00-\x1f\x7f]/g, " ");
        const text = safe.slice(0, Math.min(2048, budget));
        budget -= text.length;
        return text;
      }
      if (
        value == null ||
        typeof value === "boolean" ||
        typeof value === "number"
      )
        return value;
      if (Array.isArray(value))
        return value.slice(0, 32).map((item) => clean(item, depth + 1));
      if (!record(value)) return null;
      const out = {};
      for (const [key, item] of Object.entries(value).slice(0, 64)) {
        if (
          ["__proto__", "constructor", "prototype"].includes(key) ||
          budget <= 0 ||
          nodes <= 0
        )
          continue;
        const safeKey = clean(key);
        out[safeKey] =
          /token|secret|password|authorization|cookie|api.?key/i.test(key)
            ? "[redacted]"
            : clean(item, depth + 1);
      }
      return out;
    };
    const detail = clean(native);
    err.detail = detail;
    if (record(detail)) {
      if (Number.isInteger(detail.code) || typeof detail.code === "string")
        err.rpcCode = detail.code;
      if (Object.hasOwn(detail, "data")) err.data = detail.data;
    }
    const message =
      typeof detail === "string"
        ? detail
        : (detail?.message ?? detail?.detail?.message ?? detail?.detail);
    if (typeof message === "string" && message)
      err.message = `Hermes: ${message.slice(0, 2048)}`;
    return err;
  }
  // Redact the minted credential even if a server echoes it in JSON or errors.
  #decode(raw) {
    // Normalize JSON escapes first so an escaped echo cannot bypass redaction.
    try {
      const value = JSON.parse(raw);
      return this.#token
        ? JSON.parse(
            JSON.stringify(value).split(this.#token).join("[redacted]"),
          )
        : value;
    } catch {
      throw failure("INVALID_JSON");
    }
  }

  start() {
    if (this.#terminal) return Promise.reject(failure(this.#terminal));
    if (this.#start) return this.#start;
    let resolve;
    this.#start = new Promise((yes, no) => {
      resolve = yes;
      this.#startupReject = no;
    });
    this.#token = randomBytes(32).toString("hex");
    this.#startupTimer = setTimeout(
      () => this.#stop("STARTUP_TIMEOUT"),
      LIMITS.startupMs,
    );
    try {
      const env = {
        ...process.env,
        ...this.#env,
        HERMES_DASHBOARD_SESSION_TOKEN: this.#token,
        PYTHONUNBUFFERED: "1",
      };
      if (!Object.hasOwn(this.#env, "HERMES_DESKTOP"))
        delete env.HERMES_DESKTOP;
      // Do not inherit another desktop owner's readiness-file destination.
      delete env.HERMES_DASHBOARD_READY_FILE;
      delete env.HERMES_DESKTOP_READY_FILE;
      // The native watchdog accepts PID-only mode; never inherit another owner's identity.
      env.HERMES_PARENT_PID = String(process.pid);
      delete env.HERMES_PARENT_START_MARKER;
      delete env.HERMES_PARENT_NONCE;
      const args = [
        ...(this.#profile === "default" ? [] : ["--profile", this.#profile]),
        "serve",
        "--host",
        "127.0.0.1",
        "--port",
        "0",
      ];
      // Like Hermes Desktop: new chats fall back to the home directory, not the app's cwd.
      this.#child = this.#spawn(this.#executable, args, {
        env,
        cwd: homedir(),
        shell: false,
        stdio: ["ignore", "pipe", "ignore"],
      });
      const child = this.#child;
      child.on("error", () => this.#stop("CHILD_ERROR"));
      child.once("exit", () => this.#stop("CHILD_EXIT"));
      let line = "";
      child.stdout.on("data", (chunk) => {
        if (this.#terminal || this.#port) return;
        // Bound the partial line without concatenating arbitrarily large chunks.
        for (const byte of Buffer.from(chunk)) {
          if (byte !== 10) {
            if (line.length >= LIMITS.lineBytes) {
              this.#stop("STARTUP_OUTPUT_TOO_LARGE");
              return;
            }
            line += String.fromCharCode(byte);
            continue;
          }
          const match =
            /^HERMES_(?:BACKEND|DASHBOARD)_READY port=(\d+)\r?$/.exec(line);
          line = "";
          if (!match) continue;
          const port = Number(match[1]);
          if (!Number.isInteger(port) || port < 1 || port > 65535) {
            this.#stop("INVALID_PORT");
            return;
          }
          this.#port = port;
          clearTimeout(this.#startupTimer);
          this.#startupReject = undefined;
          resolve();
          this.#state("ready");
          return;
        }
      });
      this.#state("starting");
    } catch {
      this.#stop("CHILD_ERROR");
    }
    return this.#start;
  }

  #stop(code) {
    if (this.#terminal) return;
    this.#terminal = code;
    clearTimeout(this.#startupTimer);
    clearTimeout(this.#reconnectTimer);
    this.#reconnectTimer = undefined;
    this.#startupReject?.(failure(code));
    this.#startupReject = undefined;
    for (const op of [...this.#operations]) op.cancel(code);
    for (const channel of [...this.#sockets.values()]) channel.drop(code);
    const child = this.#child;
    if (child && child.exitCode == null && child.signalCode == null) {
      try {
        child.kill("SIGTERM");
      } catch {
        /* Already gone. */
      }
      const timer = setTimeout(() => {
        if (child.exitCode == null && child.signalCode == null) {
          try {
            child.kill("SIGKILL");
          } catch {
            /* Already gone. */
          }
        }
      }, LIMITS.killMs);
      timer.unref?.();
      child.once("exit", () => clearTimeout(timer));
    }
    this.#port = undefined;
    this.#state(code === "CLOSED" ? "closed" : "error", { code });
  }

  close() {
    const child = this.#child;
    if (!child || child.exitCode != null || child.signalCode != null) {
      this.#stop("CLOSED");
      return Promise.resolve();
    }
    const exited = new Promise((resolve) => {
      const timer = setTimeout(resolve, LIMITS.killMs + 1000);
      child.once("exit", () => {
        clearTimeout(timer);
        resolve();
      });
    });
    this.#stop("CLOSED");
    return exited;
  }

  #operation(work, timeoutMs = LIMITS.requestMs) {
    try {
      this.#check();
    } catch (err) {
      return Promise.reject(err);
    }
    if (this.#operations.size >= LIMITS.pending)
      return Promise.reject(failure("TOO_MANY_REQUESTS"));
    return new Promise((resolve, reject) => {
      const controller = new AbortController();
      let done = false,
        activeTimer;
      const finish = (err, value) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        clearTimeout(activeTimer);
        this.#operations.delete(op);
        controller.abort();
        if (err) reject(err);
        else resolve(value);
      };
      const op = { cancel: (code) => finish(failure(code)) };
      const timer = setTimeout(
        () => op.cancel("REQUEST_TIMEOUT"),
        LIMITS.startupMs + timeoutMs,
      );
      this.#operations.add(op);
      Promise.resolve()
        .then(async () => {
          await this.start();
          if (done) return;
          // Startup has its own deadline; the operation gets 30s after readiness.
          clearTimeout(timer);
          activeTimer = setTimeout(
            () => op.cancel("REQUEST_TIMEOUT"),
            timeoutMs,
          );
          try {
            finish(null, await work(controller.signal));
          } finally {
            clearTimeout(activeTimer);
          }
        })
        .catch((err) =>
          finish(transportErrors.has(err) ? err : failure("TRANSPORT_ERROR")),
        );
    });
  }

  request(
    method,
    path,
    { profile, body, query, timeoutMs = LIMITS.requestMs } = {},
  ) {
    let url, payload;
    try {
      if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 600000)
        throw failure("INVALID_TIMEOUT");
      if (!/^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)$/.test(method))
        throw failure("INVALID_METHOD");
      if (
        typeof path !== "string" ||
        !path.startsWith("/api/") ||
        /[\\\x00-\x20\x7f#]/.test(path)
      )
        throw failure("INVALID_PATH");
      url = new URL(path, "http://127.0.0.1");
      if (
        url.origin !== "http://127.0.0.1" ||
        !url.pathname.startsWith("/api/")
      )
        throw failure("INVALID_PATH");
      for (const scoped of [
        profile,
        ...url.searchParams.getAll("profile"),
        query?.profile,
        body?.profile,
      ]) {
        if (scoped !== undefined && profileName(scoped) !== this.#profile)
          throw failure("PROFILE_MISMATCH");
      }
      if (query != null) {
        if (!record(query)) throw failure("INVALID_QUERY");
        for (const [key, value] of Object.entries(query)) {
          if (value == null) continue;
          if (!["string", "number", "boolean"].includes(typeof value))
            throw failure("INVALID_QUERY");
          url.searchParams.set(key, String(value));
        }
      }
      // profile is authoritative; remove duplicate/conflicting query values.
      url.searchParams.set("profile", profileName(profile ?? this.#profile));
      if (
        [...url.searchParams.keys()].some((key) =>
          /^(token|internal|ticket)$/i.test(key),
        )
      )
        throw failure("INVALID_QUERY");
      if (Buffer.byteLength(url.href) > LIMITS.urlBytes)
        throw failure("URL_TOO_LARGE");
      if (body !== undefined) {
        if (method === "GET" || method === "HEAD")
          throw failure("INVALID_BODY");
        payload = encode(body);
      }
    } catch (err) {
      return Promise.reject(err?.code ? err : failure("INVALID_REQUEST"));
    }
    return this.#operation(async (signal) => {
      url.port = String(this.#port);
      const response = await this.#fetch(url.href, {
        method,
        body: payload,
        signal,
        redirect: "error",
        credentials: "omit",
        headers: {
          "X-Hermes-Session-Token": this.#token,
          Accept: "application/json",
          ...(payload === undefined
            ? {}
            : { "Content-Type": "application/json" }),
        },
      });
      if (signal.aborted) {
        void response.body?.cancel().catch(() => {});
        throw failure("CLOSED");
      }
      if (
        response.redirected ||
        (response.status >= 300 && response.status < 400) ||
        (response.url && response.url !== url.href)
      ) {
        void response.body?.cancel().catch(() => {});
        throw failure("REDIRECT_FORBIDDEN");
      }
      const httpError = !response.ok
        ? failure(`HTTP_${response.status}`)
        : null;
      if (httpError) httpError.status = response.status;
      if (response.status === 204 || method === "HEAD") {
        void response.body?.cancel().catch(() => {});
        return null;
      }
      const maxBytes = httpError ? LIMITS.errorBytes : LIMITS.messageBytes;
      if (Number(response.headers.get("content-length")) > maxBytes) {
        void response.body?.cancel().catch(() => {});
        throw httpError || failure("MESSAGE_TOO_LARGE");
      }
      if (!response.body?.getReader) throw failure("INVALID_RESPONSE");
      const reader = response.body.getReader();
      const abort = () => {
        void reader.cancel().catch(() => {});
      };
      signal.addEventListener("abort", abort, { once: true });
      let size = 0;
      const chunks = [];
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (signal.aborted) throw failure("CLOSED");
          if (done) break;
          size += value.byteLength;
          if (size > maxBytes) throw httpError || failure("MESSAGE_TOO_LARGE");
          chunks.push(Buffer.from(value));
        }
        let value;
        try {
          value = this.#decode(Buffer.concat(chunks).toString("utf8"));
        } catch (err) {
          throw httpError || err;
        }
        if (httpError)
          throw this.#nativeError(httpError.code, value, response.status);
        return value;
      } finally {
        signal.removeEventListener("abort", abort);
        abort();
      }
    }, timeoutMs);
  }

  #channel(profile) {
    this.#check();
    if (this.#sockets.has(profile)) return this.#sockets.get(profile);
    const url = new URL(`ws://127.0.0.1:${this.#port}/api/ws`);
    url.searchParams.set("token", this.#token);
    const socket = this.#socketFactory(url.href);
    const channel = {
      socket,
      pending: new Map(),
      closed: false,
      hold: null,
      heartbeat: null,
      stable: null,
    };
    let accept, reject;
    channel.ready = new Promise((yes, no) => {
      accept = yes;
      reject = no;
    });
    // A connection can fail before the first caller starts awaiting it.
    channel.ready.catch(() => {});
    const timer = setTimeout(
      () => channel.drop("WS_CONNECT_TIMEOUT"),
      LIMITS.connectMs,
    );
    channel.drop = (code) => {
      if (channel.closed) return;
      channel.closed = true;
      clearTimeout(timer);
      clearInterval(channel.heartbeat);
      clearTimeout(channel.stable);
      this.#sockets.delete(profile);
      reject(failure(code));
      for (const pending of [...channel.pending.values()])
        pending.reject(failure(code));
      channel.pending.clear();
      try {
        socket.close();
      } catch {
        /* Already gone. */
      }
      this.#state("disconnected", { profile, code });
      if (
        !this.#terminal &&
        this.#everOpened &&
        !this.#reconnectTimer &&
        this.#attempts < LIMITS.reconnectAttempts
      ) {
        const delay = Math.min(
          LIMITS.reconnectMaxMs,
          LIMITS.reconnectBaseMs * 2 ** this.#attempts++,
        );
        this.#reconnectTimer = setTimeout(() => {
          this.#reconnectTimer = undefined;
          if (!this.#terminal) this.#channel(profile).ready.catch(() => {});
        }, delay);
        this.#reconnectTimer.unref?.();
      }
    };
    this.#sockets.set(profile, channel);
    socket.addEventListener("open", () => {
      if (channel.closed) return;
      clearTimeout(timer);
      accept();
      this.#state("connected", { profile });
      channel.stable = setTimeout(() => {
        this.#attempts = 0;
      }, LIMITS.stableMs);
      channel.stable.unref?.();
    });
    socket.addEventListener("error", () => channel.drop("WS_ERROR"));
    socket.addEventListener("close", () => channel.drop("WS_CLOSED"));
    socket.addEventListener("message", ({ data }) => {
      if (channel.closed) return;
      if (
        typeof data !== "string" ||
        Buffer.byteLength(data) > LIMITS.messageBytes
      ) {
        channel.drop("INVALID_WS_MESSAGE");
        return;
      }
      let frame;
      try {
        frame = this.#decode(data);
      } catch {
        channel.drop("INVALID_JSON");
        return;
      }
      if (!record(frame) || frame.jsonrpc !== "2.0") {
        channel.drop("INVALID_WS_MESSAGE");
        return;
      }
      if (Object.hasOwn(frame, "id")) {
        const pending = channel.pending.get(frame.id);
        if (!pending) return;
        if (Object.hasOwn(frame, "error")) {
          const err = this.#nativeError("RPC_ERROR", frame.error);
          pending.reject(err);
        } else if (Object.hasOwn(frame, "result"))
          pending.resolve(frame.result);
        else channel.drop("INVALID_WS_MESSAGE");
      } else if (frame.method === "event" && record(frame.params)) {
        const event = { ...frame.params, profile };
        if (event.type === "gateway.ready") {
          const epoch = event.payload?.replay_epoch;
          const changed = this.#epoch && epoch && this.#epoch !== epoch;
          this.#epoch = epoch || this.#epoch;
          this.#notify(this.#onEvent, event);
          if (event.payload?.heartbeat && !channel.heartbeat) {
            let pinging = false;
            channel.heartbeat = setInterval(async () => {
              if (pinging || channel.closed) return;
              pinging = true;
              const deadline = setTimeout(
                () => channel.drop("HEARTBEAT_TIMEOUT"),
                LIMITS.heartbeatTimeoutMs,
              );
              try {
                await this.rpc(profile, "gateway.ping", {});
              } catch {
                if (!channel.closed) channel.drop("HEARTBEAT_FAILED");
              } finally {
                clearTimeout(deadline);
                pinging = false;
              }
            }, LIMITS.heartbeatMs);
            channel.heartbeat.unref?.();
          }
          if (changed) {
            for (const sid of this.#sequences.keys())
              this.#notify(this.#onEvent, {
                type: "transport.resync",
                session_id: sid,
                profile,
                payload: { reason: "backend-restarted" },
              });
            this.#sequences.clear();
          }
          if (this.#everOpened && !changed) void this.#replay(channel, profile);
          this.#everOpened = true;
        } else if (
          channel.hold &&
          event.session_id &&
          Number.isSafeInteger(event.seq)
        ) {
          if (
            channel.hold.length >= LIMITS.holdEvents ||
            (channel.holdBytes =
              (channel.holdBytes || 0) +
              Buffer.byteLength(JSON.stringify(event))) > LIMITS.holdBytes
          )
            channel.drop("REPLAY_OVERFLOW");
          else channel.hold.push(event);
        } else this.#deliver(event);
      }
    });
    return channel;
  }

  #deliver(event) {
    if (event.session_id && Number.isSafeInteger(event.seq)) {
      const previous = this.#sequences.get(event.session_id) || 0;
      if (event.seq <= previous) return;
      if (
        this.#sequences.size >= LIMITS.sessions &&
        !this.#sequences.has(event.session_id)
      )
        this.#sequences.delete(this.#sequences.keys().next().value);
      this.#sequences.set(event.session_id, event.seq);
    }
    this.#notify(this.#onEvent, event);
  }
  async #replay(channel, profile) {
    if (channel.hold) return;
    channel.hold = [];
    channel.holdBytes = 0;
    try {
      for (const [sid, seq] of [...this.#sequences]) {
        const replay = await this.rpc(profile, "session.events.since", {
          session_id: sid,
          last_seen: seq,
        });
        if (channel.closed) return;
        if (
          replay.truncated ||
          (replay.epoch && this.#epoch && replay.epoch !== this.#epoch)
        ) {
          this.#notify(this.#onEvent, {
            type: "transport.resync",
            session_id: sid,
            profile,
            payload: { reason: "history-gap" },
          });
        }
        for (const frame of (replay.events || []).slice(
          0,
          LIMITS.replayEvents,
        )) {
          const event = frame.params || frame;
          if (event.session_id === sid) this.#deliver({ ...event, profile });
        }
      }
    } catch {
      if (!channel.closed) channel.drop("REPLAY_FAILED");
    } finally {
      const held = channel.hold || [];
      channel.hold = null;
      if (!channel.closed) for (const event of held) this.#deliver(event);
      if (!channel.closed)
        for (const sid of this.#sequences.keys())
          this.#notify(this.#onEvent, {
            type: "transport.reconnected",
            session_id: sid,
            profile,
            payload: {},
          });
    }
  }

  rpc(profile, method, params = {}) {
    let name, encoded, id;
    try {
      name = profileName(profile ?? this.#profile);
      if (name !== this.#profile) throw failure("PROFILE_MISMATCH");
      if (
        typeof method !== "string" ||
        !/^[a-zA-Z][\w.-]{0,127}$/.test(method) ||
        !record(params)
      )
        throw failure("INVALID_RPC");
      id = String(++this.#nextId);
      encoded = encode({
        jsonrpc: "2.0",
        id,
        method,
        params: { ...params, profile: name },
      });
    } catch (err) {
      return Promise.reject(err);
    }
    return this.#operation(async (signal) => {
      const channel = this.#channel(name);
      await channel.ready;
      if (signal.aborted) throw failure("CLOSED");
      if (channel.closed) throw failure("WS_CLOSED");
      return new Promise((resolve, reject) => {
        const finish = (err, result) => {
          channel.pending.delete(id);
          signal.removeEventListener("abort", abort);
          if (err) reject(err);
          else resolve(result);
        };
        const abort = () => finish(failure("REQUEST_CANCELLED"));
        signal.addEventListener("abort", abort, { once: true });
        channel.pending.set(id, {
          resolve: (value) => finish(null, value),
          reject: (err) => finish(err),
        });
        try {
          if (
            socketBuffered(channel.socket) + Buffer.byteLength(encoded) >
            LIMITS.messageBytes
          )
            throw failure("WS_BACKPRESSURE");
          channel.socket.send(encoded);
        } catch {
          channel.drop("WS_SEND_FAILED");
        }
      });
    });
  }
}

function socketBuffered(socket) {
  return Number(socket.bufferedAmount) || 0;
}
module.exports = { HermesTransport, LIMITS };
