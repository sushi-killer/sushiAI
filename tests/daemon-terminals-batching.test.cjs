const test = require("node:test");
const assert = require("node:assert/strict");
const { createTerminalHandlers } = require("../electron/daemon/terminals.cjs");
const {
  OUTPUT_CHUNK_BYTES,
  OUTPUT_CREDIT_BYTES,
} = require("../electron/terminal-flow.cjs");
const { createTerminalInput } = require("../src/terminal-output.ts");
const { until, turn } = require("./helpers/deadline.cjs");

// One attached panel on a fake host. A request is answered at once, or, with
// `manual`, when the test settles it: typed input is echoed back as the
// request is answered. The renderer side acks what it receives on the next
// turn, as xterm would.
async function setup({ manual = false } = {}) {
  const sent = [];
  const requests = [];
  let onBytes;
  const manager = {
    async attach(_host, _id, _options, callback) {
      onBytes = callback;
      callback(Buffer.from("screen"), { snapshot: true });
      return { cols: 80, rows: 24, seq: 6, detach: async () => {} };
    },
    request: (_host, method, params) =>
      new Promise((resolve) => {
        const answer = () => {
          if (method === "session.input") onBytes(Buffer.from(params.data));
          resolve({});
        };
        if (manual) requests.push({ method, params, answer });
        else answer();
      }),
  };
  const handlers = createTerminalHandlers({
    getManager: () => manager,
    send: (_channel, value) => {
      sent.push(value);
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
  await until(
    () => sent.some((message) => message.snapshot !== undefined),
    "the snapshot",
  );
  return {
    handlers,
    requests,
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
  await until(
    () => text(t.data()).length >= lines.join("").length,
    "all lines",
  );
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
  await until(() => text(t.data()).length >= flood.length, "the whole flood");
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

test("typing sends every key before any answer arrives, and each echoes", async () => {
  const t = await setup({ manual: true });
  const errors = [];
  const input = createTerminalInput(
    (data) => t.handlers.write("p1", data),
    (message) => errors.push(message),
  );
  const keys = "abcdefghijklmnopqrst";
  for (const key of keys) input.send(key);
  await turn();
  // Nothing is answered yet: a write that waited for the previous answer
  // would have put out only the first key.
  const inputs = t.requests.filter((r) => r.method === "session.input");
  assert.deepEqual(
    inputs.map((r) => r.params.data),
    [...keys],
  );
  for (const request of inputs) request.answer();
  await until(() => text(t.data()).length >= keys.length, "every echo");
  assert.equal(text(t.data()), keys);
  assert.deepEqual(errors, []);
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
  await until(
    () => sent.some((message) => message.data !== undefined),
    "the first window of output",
  );
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
  await turn();
  assert.ok(
    !t.sent.some((message) => message.exited),
    "the exit must not overtake output still waiting for credit",
  );
  await until(() => {
    t.ackAll();
    return t.received.some((m) => m.exited);
  }, "the exit");
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
