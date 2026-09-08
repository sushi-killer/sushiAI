"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { HermesTransport, LIMITS } = require("../electron/agents/hermes-transport.cjs");

const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const wireEvent = (socket,type,payload={},extra={})=>socket.emit("message",JSON.stringify({jsonrpc:"2.0",method:"event",params:{type,payload,...extra}}));
class Socket extends EventTarget {
  sent = []; bufferedAmount = 0; closes = 0;
  emit(type, data) { const e = new Event(type); e.data = data; this.dispatchEvent(e); }
  send(value) { this.sent.push(JSON.parse(value)); }
  close() { this.closes++; this.emit("close"); }
  reply(index, result) { this.emit("message", JSON.stringify({ jsonrpc: "2.0", id: this.sent[index].id, result })); }
}
function fixture(t, options = {}) {
  const children = [], sockets = [], calls = [], states = [], events = [], fetches = [];
  const transport = new HermesTransport({
    onEvent: (e) => events.push(e), onState: (s) => states.push(s),
    spawn: (...args) => {
      calls.push(args);
      const child = new EventEmitter(); child.stdout = new EventEmitter(); child.kills = [];
      child.kill = (signal) => { child.kills.push(signal); child.signalCode = signal; child.emit("exit", null, signal); };
      children.push(child);
      return child;
    },
    fetcher: async (...args) => { fetches.push(args); return new Response('{"ok":true}'); },
    socketFactory: (url) => { const socket = new Socket(); socket.url = url; sockets.push(socket); return socket; },
    ...options,
  });
  t.after(() => transport.close());
  return { transport, children, sockets, calls, states, events, fetches,
    ready: async () => { const p = transport.start(); children[0].stdout.emit("data", "HERMES_BACKEND_READY port=32123\n"); await p; } };
}

test("reconnect replays missing events before held live deltas without resending prompts",async t=>{
  const f=fixture(t);await f.ready();
  const first=f.transport.rpc("default","session.create",{});await flush();const a=f.sockets[0];a.emit("open");await flush();
  wireEvent(a,"gateway.ready",{replay_epoch:"epoch-one"});a.reply(0,{session_id:"s"});await first;
  wireEvent(a,"message.delta",{text:"one"},{session_id:"s",seq:1});
  a.emit("close");
  const second=f.transport.rpc("default","gateway.ping",{});await flush();const b=f.sockets[1];b.emit("open");await flush();
  wireEvent(b,"gateway.ready",{replay_epoch:"epoch-one"});await flush();
  const replay=b.sent.findIndex(x=>x.method==="session.events.since");assert.notEqual(replay,-1);
  wireEvent(b,"message.delta",{text:"three"},{session_id:"s",seq:3});
  b.reply(replay,{epoch:"epoch-one",events:[{jsonrpc:"2.0",method:"event",params:{type:"message.delta",session_id:"s",seq:2,payload:{text:"two"}}}],truncated:false});await flush();
  b.reply(b.sent.findIndex(x=>x.method==="gateway.ping"),{ok:true});await second;
  assert.deepEqual(f.events.filter(e=>e.type==="message.delta").map(e=>e.payload.text),["one","two","three"]);
  assert.equal(b.sent.some(r=>r.method==="session.create"),false);
  assert.ok(f.events.some(e=>e.type==="transport.reconnected"));
});

test("single-flight native startup, split sentinel, owned token and isolated environment", async (t) => {
  const f = fixture(t, { env: { HERMES_HOME: "/fake/hermes-home", HERMES_DASHBOARD_SESSION_TOKEN: "not-owned",
    HERMES_DESKTOP_READY_FILE: "/never/write", HERMES_PARENT_NONCE: "stale" } });
  const p = f.transport.start(); assert.equal(f.transport.start(), p);
  const [executable, args, options] = f.calls[0];
  assert.ok(executable.endsWith("/.local/bin/hermes"));
  assert.deepEqual(args, ["serve", "--host", "127.0.0.1", "--port", "0"]);
  assert.equal(options.env.HERMES_HOME, "/fake/hermes-home");
  assert.match(options.env.HERMES_DASHBOARD_SESSION_TOKEN, /^[a-f0-9]{64}$/);
  assert.equal(options.env.HERMES_DESKTOP, undefined);
  assert.equal(options.env.HERMES_DESKTOP_READY_FILE, undefined);
  assert.equal(options.env.HERMES_PARENT_PID, String(process.pid));
  assert.equal(options.env.HERMES_PARENT_NONCE, undefined);
  assert.deepEqual(options.stdio, ["ignore", "pipe", "ignore"]);
  assert.equal(options.shell, false);
  f.children[0].stdout.emit("data", "log\nHERMES_BACKEND_RE");
  f.children[0].stdout.emit("data", "ADY port=32123\r\n"); await p;
  assert.deepEqual(f.states, [{ state: "starting" }, { state: "ready" }]);
  assert.equal(JSON.stringify(f.transport), "{}");
});

test("dedicated backend pins all RPCs to its profile", async (t) => {
  const f = fixture(t, { profile: "work" }); await f.ready();
  assert.deepEqual(f.calls[0][1].slice(0, 3), ["--profile", "work", "serve"]);
  await assert.rejects(f.transport.rpc("other", "config.get"), { code: "PROFILE_MISMATCH" });
  assert.equal(f.sockets.length, 0);
  const p = f.transport.rpc("work", "session.create", { profile: "other" }); await flush();
  const s = f.sockets[0]; const url = new URL(s.url);
  assert.equal(url.pathname, "/api/ws"); assert.equal(url.searchParams.get("profile"), null);
  assert.equal(url.searchParams.get("token"), f.calls[0][2].env.HERMES_DASHBOARD_SESSION_TOKEN);
  s.emit("open"); await flush(); assert.equal(s.sent[0].params.profile, "work");
  s.reply(0, { session_id: "s1" }); assert.deepEqual(await p, { session_id: "s1" });
  s.emit("message", JSON.stringify({ jsonrpc: "2.0", method: "event", params: { type: "token", seq: 7, profile: "wrong" } }));
  assert.deepEqual(f.events, [{ type: "token", seq: 7, profile: "work" }]);
});

test("HTTP authenticated JSON, canonical profile query, explicit scope and body", async (t) => {
  const f = fixture(t, { profile: "work" }); await f.ready();
  assert.deepEqual(await f.transport.request("POST", "/api/config?profile=work&profile=work", {
    profile: "work", query: { profile: "work", q: "a & b", count: 2 }, body: { profile: "work", x: 1 },
  }), { ok: true });
  const [raw, opts] = f.fetches[0], url = new URL(raw);
  assert.equal(url.origin, "http://127.0.0.1:32123");
  assert.deepEqual(url.searchParams.getAll("profile"), ["work"]);
  assert.equal(url.searchParams.get("q"), "a & b");
  assert.equal(opts.redirect, "error"); assert.equal(opts.credentials, "omit");
  assert.equal(opts.headers["X-Hermes-Session-Token"], f.calls[0][2].env.HERMES_DASHBOARD_SESSION_TOKEN);
  assert.deepEqual(JSON.parse(opts.body), { profile: "work", x: 1 });
  await assert.rejects(f.transport.request("GET", "/api/status", { profile: "another" }), /PROFILE_MISMATCH/);
  await f.transport.request("GET", "/api/status");
  assert.equal(new URL(f.fetches[1][0]).searchParams.get("profile"), "work");
});

test("invalid URLs and oversized requests never launch or fetch", async (t) => {
  const f = fixture(t);
  for (const path of ["https://evil/api/x", "//evil/api/x", "/api/../../secret", "/api/\\evil", "/api/x#fragment", "/api/x?token=bad"]) {
    await assert.rejects(f.transport.request("GET", path));
  }
  await assert.rejects(f.transport.request("POST", "/api/x", { body: "x".repeat(LIMITS.messageBytes) }), { code: "MESSAGE_TOO_LARGE" });
  await assert.rejects(f.transport.rpc("default", "session.create", { huge: "x".repeat(LIMITS.messageBytes) }), { code: "MESSAGE_TOO_LARGE" });
  assert.equal(f.calls.length, 0);
});

test("HTTP bounds streamed bodies and rejects malformed JSON and redirects", async (t) => {
  const responses = [
    new Response("oops"), new Response(null, { status: 302 }), new Response("secret", { status: 401 }),
    new Response("x", { headers: { "content-length": String(LIMITS.messageBytes + 1) } }),
    new Response(new ReadableStream({ start(c) { c.enqueue(new Uint8Array(LIMITS.messageBytes)); c.enqueue(new Uint8Array(1)); c.close(); } })),
    new Response(null, { status: 204 }),
  ];
  const f = fixture(t, { fetcher: async () => responses.shift() }); await f.ready();
  for (const code of ["INVALID_JSON", "REDIRECT_FORBIDDEN", "HTTP_401", "MESSAGE_TOO_LARGE", "MESSAGE_TOO_LARGE"]) {
    await assert.rejects(f.transport.request("GET", "/api/status"), { code });
  }
  assert.equal(await f.transport.request("DELETE", "/api/x"), null);
});

test("RPC IDs correlate out-of-order replies, propagate codes, and share connection", async (t) => {
  const f = fixture(t); await f.ready();
  const a = f.transport.rpc("default", "one"), b = f.transport.rpc("default", "two");
  await flush(); assert.equal(f.sockets.length, 1);
  const s = f.sockets[0]; s.emit("open"); await flush();
  s.reply(1, 2); s.reply(0, 1); assert.deepEqual(await Promise.all([a, b]), [1, 2]);
  const p = f.transport.rpc("default", "bad"); await flush();
  const check = assert.rejects(p, { code: "RPC_ERROR", rpcCode: -32601 });
  s.emit("message", JSON.stringify({ jsonrpc: "2.0", id: s.sent[2].id, error: { code: -32601, message: "secret" } }));
  await check;
});

test("disconnect rejects pending RPC without resend; next call reconnects", async (t) => {
  const f = fixture(t); await f.ready();
  const p = f.transport.rpc("default", "session.create"); await flush();
  f.sockets[0].emit("open"); await flush();
  const check = assert.rejects(p, { code: "WS_CLOSED" }); f.sockets[0].emit("close"); await check;
  const next = f.transport.rpc("default", "session.events.since", { session_id: "s", last_seen: 7 }); await flush();
  assert.equal(f.sockets.length, 2); f.sockets[1].emit("open"); await flush();
  assert.equal(f.sockets[1].sent.length, 1); assert.equal(f.sockets[1].sent[0].method, "session.events.since");
  f.sockets[1].reply(0, { events: [] }); await next;
});

test("malformed, binary, oversized WS frames fail pending calls", async (t) => {
  for (const data of ["no-json", "[]", new Uint8Array(1), "x".repeat(LIMITS.messageBytes + 1)]) {
    const f = fixture(t); await f.ready(); const p = f.transport.rpc("default", "ping"); await flush();
    f.sockets[0].emit("open"); await flush(); const check = assert.rejects(p);
    f.sockets[0].emit("message", data); await check; assert.equal(f.sockets[0].closes, 1);
  }
});

test("close cancels HTTP even when fetch ignores abort; owned child only and idempotent", async (t) => {
  let signal;
  const f = fixture(t, { fetcher: (_url, opts) => { signal = opts.signal; return new Promise(() => {}); } }); await f.ready();
  const p = f.transport.request("GET", "/api/status"); await flush();
  const check = assert.rejects(p, { code: "CLOSED" }); f.transport.close(); f.transport.close(); await check;
  assert.equal(signal.aborted, true); assert.deepEqual(f.children[0].kills, ["SIGTERM"]);
  await assert.rejects(f.transport.start(), { code: "CLOSED" });
});

test("close during startup rejects waiters; child errors and exits are sanitized", async (t) => {
  for (const kind of ["close", "error", "exit"]) {
    const f = fixture(t); const p = f.transport.start();
    const check = assert.rejects(p, { code: kind === "close" ? "CLOSED" : kind === "error" ? "CHILD_ERROR" : "CHILD_EXIT" });
    if (kind === "close") f.transport.close();
    else f.children[0].emit(kind, new Error("secret stdout token"));
    await check; assert.ok(!JSON.stringify(f.states).includes("secret"));
  }
});

test("invalid port and unbounded startup line fail safely", async (t) => {
  for (const [data, code] of [["HERMES_BACKEND_READY port=65536\n", "INVALID_PORT"], ["x".repeat(LIMITS.lineBytes + 1), "STARTUP_OUTPUT_TOO_LARGE"]]) {
    const f = fixture(t); const check = assert.rejects(f.transport.start(), { code });
    f.children[0].stdout.emit("data", data); await check;
  }
});

test("bounded admission during startup", async (t) => {
  const f = fixture(t);
  const pending = Array.from({ length: LIMITS.pending }, () => f.transport.request("GET", "/api/status"));
  const checks = pending.map((p) => assert.rejects(p, { code: "CLOSED" }));
  await assert.rejects(f.transport.request("GET", "/api/status"), { code: "TOO_MANY_REQUESTS" });
  f.transport.close(); await Promise.all(checks);
});

test("timeouts cover startup, connecting WS, and unresponsive HTTP", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const f = fixture(t); const check = assert.rejects(f.transport.start(), { code: "STARTUP_TIMEOUT" });
  t.mock.timers.tick(LIMITS.startupMs); await check;
  const g = fixture(t); await g.ready(); const rpc = g.transport.rpc("default", "ping"); await flush();
  const rpcCheck = assert.rejects(rpc, { code: "WS_CONNECT_TIMEOUT" });
  t.mock.timers.tick(LIMITS.connectMs); await rpcCheck;
  const h = fixture(t, { fetcher: () => new Promise(() => {}) }); await h.ready();
  const req = h.transport.request("GET", "/api/status"); await flush();
  const reqCheck = assert.rejects(req, { code: "REQUEST_TIMEOUT" });
  t.mock.timers.tick(LIMITS.requestMs); await reqCheck;
});

test("minted token never appears in returned payloads or callbacks", async (t) => {
  let token;
  const f = fixture(t, { fetcher: async () => new Response(JSON.stringify({ echoed: token })) }); await f.ready();
  token = f.calls[0][2].env.HERMES_DASHBOARD_SESSION_TOKEN;
  assert.deepEqual(await f.transport.request("GET", "/api/status"), { echoed: "[redacted]" });
  const p = f.transport.rpc("default", "ping"); await flush(); f.sockets[0].emit("open"); await flush();
  f.sockets[0].reply(0, token); assert.equal(await p, "[redacted]");
  const escaped = token.split("").map((c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`).join("");
  f.sockets[0].emit("message", `{"jsonrpc":"2.0","method":"event","params":{"type":"${escaped}"}}`);
  assert.equal(f.events[0].type, "[redacted]");
});

test("ready child failure cancels active work and closes its socket", async (t) => {
  const f = fixture(t); await f.ready();
  const p = f.transport.rpc("default", "ping"); await flush(); f.sockets[0].emit("open"); await flush();
  const check = assert.rejects(p, { code: "CHILD_ERROR" });
  f.children[0].emit("error", new Error("private diagnostic")); await check;
  assert.equal(f.sockets[0].closes, 1);
  await assert.rejects(f.transport.rpc("default", "ping"), { code: "CHILD_ERROR" });
});

test("RPC timeout releases slot and ignores late replies", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const f = fixture(t); await f.ready();
  const p = f.transport.rpc("default", "ping"); await flush(); f.sockets[0].emit("open"); await flush();
  const check = assert.rejects(p, { code: "REQUEST_TIMEOUT" }); t.mock.timers.tick(LIMITS.requestMs); await check;
  f.sockets[0].reply(0, "too late");
  const next = f.transport.rpc("default", "ping"); await flush();
  f.sockets[0].reply(1, "ok"); assert.equal(await next, "ok");
});

test("stalled HTTP stream is cancelled at deadline", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let cancelled = false;
  const f = fixture(t, { fetcher: async () => new Response(new ReadableStream({ cancel() { cancelled = true; } })) });
  await f.ready(); const p = f.transport.request("GET", "/api/status"); await flush();
  const check = assert.rejects(p, { code: "REQUEST_TIMEOUT" }); t.mock.timers.tick(LIMITS.requestMs); await check;
  assert.equal(cancelled, true);
});

test("close during WS connect rejects caller and ignores late open", async (t) => {
  const f = fixture(t); await f.ready(); const p = f.transport.rpc("default", "ping"); await flush();
  const check = assert.rejects(p, { code: "CLOSED" }); f.transport.close(); await check;
  f.sockets[0].emit("open"); assert.equal(f.sockets[0].sent.length, 0);
});

test("backpressure rejects instead of buffering; observer exceptions are isolated", async (t) => {
  const f = fixture(t, { onState: () => { throw new Error("observer"); }, onEvent: () => { throw new Error("observer"); } });
  await f.ready(); const p = f.transport.rpc("default", "ping"); await flush();
  f.sockets[0].bufferedAmount = LIMITS.messageBytes;
  const check = assert.rejects(p, { code: "WS_SEND_FAILED" }); f.sockets[0].emit("open"); await check;
  assert.equal(f.sockets[0].sent.length, 0);
});

test("owned child gets bounded SIGTERM to SIGKILL escalation", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const f = fixture(t); await f.ready();
  f.children[0].kill = (signal) => f.children[0].kills.push(signal);
  const closed = f.transport.close(); assert.deepEqual(f.children[0].kills, ["SIGTERM"]);
  t.mock.timers.tick(LIMITS.killMs); assert.deepEqual(f.children[0].kills, ["SIGTERM", "SIGKILL"]);
  f.children[0].signalCode = "SIGKILL"; f.children[0].emit("exit", null, "SIGKILL");
  await closed;
});

test("spawn and fetch exceptions cannot leak native error messages", async (t) => {
  const f = fixture(t, { spawn: () => { throw new Error("sensitive spawn environment"); } });
  await assert.rejects(f.transport.start(), { code: "CHILD_ERROR" });
  const g = fixture(t, { fetcher: () => { throw new Error("Hermes transport: secret-token"); } }); await g.ready();
  await assert.rejects(g.transport.request("GET", "/api/status"), { code: "TRANSPORT_ERROR" });
});

test("long native actions have an explicit bounded deadline",async t=>{
  t.mock.timers.enable({apis:["setTimeout"]});
  const f=fixture(t,{fetcher:()=>new Promise(()=>{})});await f.ready();
  let settled=false;
  const request=f.transport.request("POST","/api/cron/jobs/test/trigger",{timeoutMs:60000});
  request.then(()=>{settled=true;},()=>{settled=true;});
  const check=assert.rejects(request,{code:"REQUEST_TIMEOUT"});await flush();
  t.mock.timers.tick(LIMITS.requestMs);await flush();assert.equal(settled,false);
  t.mock.timers.tick(30000);await check;
  await assert.rejects(f.transport.request("GET","/api/status",{timeoutMs:600001}),{code:"INVALID_TIMEOUT"});
});
