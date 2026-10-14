const test = require("node:test");
const assert = require("node:assert/strict");
const { createTerminalHandlers } = require("../electron/daemon/terminals.cjs");
const {
  OUTPUT_CHUNK_BYTES,
  OUTPUT_CREDIT_BYTES,
} = require("../electron/terminal-flow.cjs");
const { createTerminalInput } = require("../src/terminal-output.ts");

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// One attached panel on a fake host. A request takes `latencyMs` to answer;
// typed input is echoed back half a round trip after it was sent. The renderer
// side acks what it receives on the next turn, as xterm would.
async function setup({ latencyMs = 0 } = {}) {
  const sent = [];
  let onBytes;
  const manager = {
    async attach(_host, _id, _options, callback) {
      onBytes = callback;
      callback(Buffer.from("screen"), { snapshot: true });
      return { cols: 80, rows: 24, seq: 6, detach: async () => {} };
    },
    request: (_host, method, params) =>
      new Promise((resolve) => {
        if (method === "session.input")
          setTimeout(() => onBytes(Buffer.from(params.data)), latencyMs / 2);
        setTimeout(() => resolve({}), latencyMs);
      }),
  };
  const handlers = createTerminalHandlers({
    getManager: () => manager,
    send: (_channel, value) => {
      sent.push({ at: performance.now(), ...value });
      const text = value.snapshot ?? value.data;
      if (text !== undefined)
        setImmediate(() =>
          handlers.ack(value.panelId, Buffer.byteLength(text)),
        );
    },
  });
  await handlers.attach({
    panelId: "p1",
    host: "h1",
    sessionId: "s1",
    cols: 80,
    rows: 24,
  });
  await wait(5);
  return {
    handlers,
    output: (buffer) => onBytes(buffer),
    data: () => sent.filter((message) => message.data !== undefined),
  };
}

const text = (messages) => messages.map((message) => message.data).join("");

test("small output frames that arrive together leave main as one message", async () => {
  const t = await setup();
  const lines = Array.from(
    { length: 200 },
    (_, i) => `line ${String(i).padStart(4, "0")} ok\r\n`,
  );
  for (const line of lines) t.output(Buffer.from(line));
  await wait(20);
  const data = t.data();
  assert.equal(text(data), lines.join(""));
  assert.ok(
    data.length <= 2,
    `200 small frames went out as ${data.length} messages`,
  );
});

test("a flood leaves main in full messages, cut on whole characters", async () => {
  const t = await setup();
  let flood = "";
  for (let i = 0; flood.length < 512 * 1024; i++)
    flood += `${i} done \u00e9 \u{1F680} \x1b[32mok\x1b[0m\r\n`;
  const bytes = Buffer.from(flood);
  for (let at = 0; at < bytes.length; at += 4096)
    t.output(bytes.subarray(at, at + 4096));
  for (let i = 0; i < 400 && text(t.data()).length < flood.length; i++)
    await wait(5);
  const data = t.data();
  assert.equal(text(data), flood);
  const limit = Math.ceil(bytes.length / OUTPUT_CHUNK_BYTES) + 2;
  assert.ok(
    data.length <= limit,
    `${bytes.length} bytes went out as ${data.length} messages, expected at most ${limit}`,
  );
  for (const { data: piece } of data) {
    assert.ok(Buffer.byteLength(piece) <= OUTPUT_CHUNK_BYTES);
    assert.ok(
      !/[\uD800-\uDBFF]$/.test(piece) && !/^[\uDC00-\uDFFF]/.test(piece),
      "a message never splits a surrogate pair",
    );
  }
});

test("typing over a 40 ms round trip echoes every key within about one round trip", async () => {
  const t = await setup({ latencyMs: 40 });
  const errors = [];
  const input = createTerminalInput(
    (data) => t.handlers.write("p1", data),
    (message) => errors.push(message),
  );
  const keys = "abcdefghijklmnopqrst";
  const typed = [];
  for (const key of keys) {
    typed.push(performance.now());
    input.send(key);
    await wait(5);
  }
  const echoed = () => {
    let seen = "";
    const times = [];
    for (const message of t.data())
      for (const character of message.data) {
        seen += character;
        times.push(message.at);
      }
    return { seen, times };
  };
  for (let i = 0; i < 300 && echoed().seen.length < keys.length; i++)
    await wait(10);
  const { seen, times } = echoed();
  assert.equal(seen, keys);
  assert.deepEqual(errors, []);
  const latencies = typed.map((at, i) => times[i] - at);
  assert.ok(
    Math.max(...latencies) < 150,
    `keystroke-to-echo latencies: ${latencies.map(Math.round).join(", ")} ms`,
  );
});

// A renderer that acks only when told to, so the credit can run out.
async function stalled() {
  const sent = [];
  let onBytes;
  let listener;
  const handlers = createTerminalHandlers({
    getManager: () => ({
      async attach(_host, _id, _options, callback) {
        onBytes = callback;
        callback(Buffer.from("screen"), { snapshot: true });
        return { cols: 80, rows: 24, detach: async () => {} };
      },
      request: async () => ({}),
    }),
    send: (_channel, value) => sent.push(value),
    onEvent: (callback) => {
      listener = callback;
      return () => {};
    },
  });
  await handlers.attach({
    panelId: "p1",
    host: "h1",
    sessionId: "s1",
    cols: 80,
    rows: 24,
  });
  const flood = "x".repeat(OUTPUT_CREDIT_BYTES + 3 * OUTPUT_CHUNK_BYTES);
  onBytes(Buffer.from(flood));
  await wait(5);
  // What the renderer took so far; acking it releases the next window.
  const received = [];
  const ackAll = () => {
    for (const message of sent.splice(0)) {
      received.push(message);
      const text = message.snapshot ?? message.data;
      if (text !== undefined) handlers.ack("p1", Buffer.byteLength(text));
    }
  };
  return { handlers, sent, received, ackAll, flood, emit: (e) => listener(e) };
}

test("an exit waits for the output queued before it while the credit is out", async () => {
  const t = await stalled();
  t.emit({ host: "h1", method: "session.exited", params: { id: "s1" } });
  await wait(5);
  assert.ok(
    !t.sent.some((message) => message.exited),
    "the exit must not overtake output still waiting for credit",
  );
  for (let i = 0; i < 20 && !t.received.some((m) => m.exited); i++) {
    t.ackAll();
    await wait(5);
  }
  const exits = t.received.filter((message) => message.exited);
  assert.equal(exits.length, 1);
  assert.ok(t.received.at(-1).exited, "the exit comes last");
  assert.equal(text(t.received.filter((m) => m.data !== undefined)), t.flood);
});

test("a host that goes away sends all queued output before the exit", async () => {
  const t = await stalled();
  await t.handlers.closeHost("h1");
  t.ackAll();
  assert.equal(t.received.filter((message) => message.exited).length, 1);
  assert.ok(t.received.at(-1).exited, "the exit comes last");
  assert.equal(text(t.received.filter((m) => m.data !== undefined)), t.flood);
});
