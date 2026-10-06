const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const {
  connectDaemon: connectRaw,
  RpcError,
} = require("../electron/daemon/client.cjs");
const { createDecoder, encode } = require("../electron/daemon/frame.cjs");

// Everything a test opens is closed after it, so a failed assertion cannot hang the run.
const cleanups = [];
test.afterEach(async () => {
  while (cleanups.length) await cleanups.pop()();
});

async function connectTracked(options) {
  const client = await connectRaw(options);
  cleanups.push(() => client.close());
  return client;
}

// In-process fake daemon on a short unix socket. `handler(conn, message)` answers
// requests after the built-in hello handling.
async function startDaemon(handler = () => {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sd-"));
  const socketPath = path.join(dir, "d.sock");
  const conns = [];
  const log = [];
  const server = net.createServer((socket) => {
    const decoder = createDecoder();
    let greeted = false;
    const conn = {
      socket,
      json: (message) =>
        socket.write(
          encode({
            kind: "J",
            json: JSON.stringify({ jsonrpc: "2.0", ...message }),
          }),
        ),
      bytes: (id, seq, text) =>
        socket.write(encode({ kind: "B", id, seq, data: Buffer.from(text) })),
    };
    conns.push(conn);
    socket.on("error", () => {});
    socket.on("data", (chunk) => {
      for (const frame of decoder.push(chunk)) {
        const message = JSON.parse(frame.json);
        log.push(message);
        if (!greeted) {
          if (message.method !== "hello") {
            conn.json({
              id: message.id,
              error: { code: 1001, message: "not initialized" },
            });
            return;
          }
          greeted = true;
          conn.json({
            id: message.id,
            result: { protocol: 1, capabilities: ["sessions"], daemon: "fake" },
          });
          continue;
        }
        handler(conn, message);
      }
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  const stop = async () => {
    for (const conn of conns) conn.socket.destroy();
    await new Promise((resolve) => server.close(resolve));
    fs.rmSync(dir, { recursive: true, force: true });
  };
  cleanups.push(stop);
  return {
    socketPath,
    conns,
    log,
    stop,
  };
}

const snapshotResult = (text, seq) => ({
  snapshot: Buffer.from(text).toString("base64"),
  seq,
  cols: 80,
  rows: 24,
});

test("hello is the first message and its result is exposed", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "test",
  });
  assert.equal(daemon.log[0].method, "hello");
  assert.deepEqual(daemon.log[0].params, { protocol: 1, client: "test" });
  assert.equal(client.hello.daemon, "fake");
  client.close();
  await daemon.stop();
});

test("protocol mismatch rejects the connection", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sd-"));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  // A daemon answering protocol 2 is simulated by patching the reply on the wire.
  const server = net.createServer((socket) => {
    const decoder = createDecoder();
    socket.on("data", (chunk) => {
      for (const frame of decoder.push(chunk)) {
        const { id } = JSON.parse(frame.json);
        const json = JSON.stringify({
          jsonrpc: "2.0",
          id,
          result: { protocol: 2, capabilities: [], daemon: "x" },
        });
        socket.write(encode({ kind: "J", json }));
      }
    });
  });
  const mismatchPath = path.join(dir, "m.sock");
  await new Promise((resolve) => server.listen(mismatchPath, resolve));
  cleanups.push(() => new Promise((resolve) => server.close(resolve)));
  await assert.rejects(
    connectTracked({ socketPath: mismatchPath, clientName: "t" }),
    /protocol 2/,
  );
});

test("hello error from the daemon rejects with an RpcError code", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sd-"));
  const socketPath = path.join(dir, "d.sock");
  const server = net.createServer((socket) => {
    const decoder = createDecoder();
    socket.on("data", (chunk) => {
      for (const frame of decoder.push(chunk)) {
        const { id } = JSON.parse(frame.json);
        socket.write(
          encode({
            kind: "J",
            json: JSON.stringify({
              jsonrpc: "2.0",
              id,
              error: { code: 1002, message: "mismatch" },
            }),
          }),
        );
      }
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  cleanups.push(() => new Promise((resolve) => server.close(resolve)));
  await assert.rejects(
    connectTracked({ socketPath, clientName: "t" }),
    (error) => error instanceof RpcError && error.code === 1002,
  );
  await new Promise((resolve) => server.close(resolve));
  fs.rmSync(dir, { recursive: true, force: true });
});

test("requests are matched to responses by id, even out of order", async () => {
  const held = [];
  const daemon = await startDaemon((conn, message) => {
    held.push({ conn, message });
    if (held.length === 2) {
      for (const { conn: c, message: m } of held.reverse())
        c.json({ id: m.id, result: { echo: m.params.n } });
    }
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const [a, b] = await Promise.all([
    client.request("session.list", { n: 1 }),
    client.request("session.list", { n: 2 }),
  ]);
  assert.deepEqual([a, b], [{ echo: 1 }, { echo: 2 }]);
  client.close();
  await daemon.stop();
});

test("an error response rejects with the daemon code", async () => {
  const daemon = await startDaemon((conn, message) =>
    conn.json({
      id: message.id,
      error: { code: 1003, message: "no such session" },
    }),
  );
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  await assert.rejects(
    client.request("session.close", { id: "nope" }),
    (error) => error instanceof RpcError && error.code === 1003,
  );
  client.close();
  await daemon.stop();
});

test("a request that gets no answer times out and a late answer is ignored", async () => {
  let late;
  const daemon = await startDaemon((conn, message) => {
    late = () => conn.json({ id: message.id, result: {} });
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  await assert.rejects(
    client.request("session.list", {}, { timeoutMs: 30 }),
    /timed out/,
  );
  assert.equal(client.pendingCount(), 0);
  late();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(client.pendingCount(), 0);
  client.close();
  await daemon.stop();
});

test("notifications are emitted by method name", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const got = new Promise((resolve) => client.once("session.exited", resolve));
  daemon.conns[0].json({
    method: "session.exited",
    params: { id: "s1", code: 3 },
  });
  assert.deepEqual(await got, { id: "s1", code: 3 });
  client.close();
  await daemon.stop();
});

test("attach delivers the snapshot first, then bytes in order, dropping stale ones", async () => {
  const daemon = await startDaemon((conn, message) => {
    if (message.method !== "session.attach") return;
    conn.bytes("s1", 0, "old-");
    conn.json({ id: message.id, result: snapshotResult("SNAP", 10) });
    conn.bytes("s1", 6, "stale!!"); // ends at 13: overlaps snapshot seq 10 by 3 bytes
    conn.bytes("s1", 13, "-next");
    conn.bytes("other", 0, "ignored");
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const chunks = [];
  const attached = await client.attach("s1", (data, info) =>
    chunks.push([data.toString(), info.snapshot]),
  );
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(attached.seq, 10n);
  assert.deepEqual(chunks, [
    ["SNAP", true],
    ["e!!", false],
    ["-next", false],
  ]);
  client.close();
  await daemon.stop();
});

test("attach drops output entirely below the snapshot seq", async () => {
  const daemon = await startDaemon((conn, message) => {
    conn.json({ id: message.id, result: snapshotResult("S", 100) });
    conn.bytes("s1", 90, "0123456789"); // ends at 100, fully stale
    conn.bytes("s1", 100, "fresh");
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const chunks = [];
  await client.attach("s1", (data) => chunks.push(data.toString()));
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(chunks, ["S", "fresh"]);
  client.close();
  await daemon.stop();
});

test("a gap in seq is reported once and delivery continues", async () => {
  const daemon = await startDaemon((conn, message) => {
    conn.json({ id: message.id, result: snapshotResult("S1", 0) });
    conn.bytes("s1", 0, "abc");
    conn.bytes("s1", 50, "gap1"); // bytes 3..50 missing
    conn.bytes("s1", 60, "gap2");
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const errors = [];
  client.on("protocol-error", (error) => errors.push(error.message));
  const chunks = [];
  await client.attach("s1", (data) => chunks.push(data.toString()));
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(errors.length, 1);
  assert.match(errors[0], /expected 3, got 50/);
  assert.deepEqual(chunks, ["S1", "abc", "gap1", "gap2"]);
});

test("throwing consumers do not tear down the connection", async () => {
  const daemon = await startDaemon((conn, message) => {
    conn.json({ id: message.id, result: snapshotResult("S", 0) });
    conn.bytes("s1", 0, "one");
    conn.json({ method: "session.exited", params: { id: "s1", code: 0 } });
    conn.bytes("s1", 3, "two");
    conn.json({ method: "session.resync", params: { sessions: [] } });
  });
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  let disconnects = 0;
  const callbackErrors = [];
  const resynced = new Promise((resolve) =>
    client.once("session.resync", resolve),
  );
  client.on("disconnect", () => (disconnects += 1));
  client.on("callback-error", (error) => callbackErrors.push(error.message));
  client.on("session.exited", () => {
    throw new Error("listener boom");
  });
  const chunks = [];
  await client.attach("s1", (data) => {
    chunks.push(data.toString());
    if (data.toString() === "one") throw new Error("bytes boom");
  });
  await resynced;
  assert.deepEqual(chunks, ["S", "one", "two"]);
  assert.deepEqual(callbackErrors.sort(), ["bytes boom", "listener boom"]);
  assert.equal(disconnects, 0);
  assert.equal((await client.request("session.list")).cols, 80);
});

test("a second attach for the same session is rejected, the first stays live", async () => {
  const daemon = await startDaemon((conn, message) =>
    conn.json({ id: message.id, result: snapshotResult("S", 0) }),
  );
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const first = [];
  const handle = await client.attach("s1", (data) =>
    first.push(data.toString()),
  );
  await assert.rejects(
    client.attach("s1", () => {}),
    /already attached/,
  );
  daemon.conns[0].bytes("s1", 0, "x");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(first, ["S", "x"]);
  await handle.detach();
  const again = [];
  await client.attach("s1", (data) => again.push(data.toString()));
  assert.deepEqual(again, ["S"]);
});

test("a detached session receives no bytes or snapshots", async () => {
  const daemon = await startDaemon((conn, message) =>
    conn.json({ id: message.id, result: snapshotResult("S", 0) }),
  );
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const chunks = [];
  const handle = await client.attach("s1", (data) =>
    chunks.push(data.toString()),
  );
  await handle.detach().catch(() => {});
  daemon.conns[0].bytes("s1", 0, "late");
  daemon.conns[0].json({
    method: "session.snapshot",
    params: { id: "s1", ...snapshotResult("S2", 9) },
  });
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(chunks, ["S"]);
});

test("an unencodable request rejects without leaking a pending entry", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  await assert.rejects(client.request("session.list", { n: 1n }), TypeError);
  assert.equal(client.pendingCount(), 0);
});

test("requests reject with backpressure when the write buffer is over 8 MiB", async () => {
  const { EventEmitter } = require("node:events");
  const socket = Object.assign(new EventEmitter(), {
    connecting: false,
    writableLength: 0,
    write() {},
    destroy() {},
  });
  socket.write = (bytes) => {
    const { id } = JSON.parse(bytes.subarray(5).toString());
    const result = { protocol: 1, capabilities: [], daemon: "stub" };
    const json = JSON.stringify({ jsonrpc: "2.0", id, result });
    socket.emit("data", encode({ kind: "J", json }));
  };
  const connected = await connectTracked({
    socketPath: "unused",
    clientName: "t",
    connect: () => socket,
  });
  socket.writableLength = 8 * 1024 * 1024 + 1;
  await assert.rejects(connected.request("session.list"), /backpressure/);
  assert.equal(connected.pendingCount(), 0);
});

test("a session.snapshot notification replaces the stream for an attached session", async () => {
  const daemon = await startDaemon((conn, message) =>
    conn.json({ id: message.id, result: snapshotResult("S1", 0) }),
  );
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const chunks = [];
  await client.attach("s1", (data, info) =>
    chunks.push([data.toString(), info.snapshot]),
  );
  const seen = new Promise((resolve) =>
    client.once("session.snapshot", resolve),
  );
  daemon.conns[0].json({
    method: "session.snapshot",
    params: { id: "s1", ...snapshotResult("S9", 500) },
  });
  await seen;
  daemon.conns[0].bytes("s1", 500, "go");
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(chunks, [
    ["S1", true],
    ["S9", true],
    ["go", false],
  ]);
  client.close();
  await daemon.stop();
});

test("a dropped connection rejects pending requests and emits disconnect", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const disconnected = new Promise((resolve) =>
    client.once("disconnect", resolve),
  );
  const inflight = client.request("session.list", {}, { timeoutMs: 5000 });
  const settled = assert.rejects(inflight, /lost|closed/);
  await new Promise((resolve) => setTimeout(resolve, 20));
  daemon.conns[0].socket.destroy();
  await settled;
  await disconnected;
  await assert.rejects(client.request("session.list"), /closed/);
  await daemon.stop();
});

test("a malformed frame from the daemon drops the connection", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  const disconnected = new Promise((resolve) =>
    client.once("disconnect", resolve),
  );
  daemon.conns[0].socket.write(Buffer.from([0, 0, 0, 1, 0x5a]));
  assert.match((await disconnected).message, /unknown frame kind/);
  await daemon.stop();
});

test("close() rejects pending requests without a disconnect event", async () => {
  const daemon = await startDaemon();
  const client = await connectTracked({
    socketPath: daemon.socketPath,
    clientName: "t",
  });
  let disconnects = 0;
  client.on("disconnect", () => (disconnects += 1));
  const inflight = client.request("session.list", {}, { timeoutMs: 5000 });
  client.close();
  await assert.rejects(inflight, /closed/);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(disconnects, 0);
  await daemon.stop();
});
