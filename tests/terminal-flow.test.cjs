const { test } = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter, once } = require("node:events");
const { spawn } = require("node:child_process");
const { PassThrough, Writable } = require("node:stream");
const {
  OUTPUT_CREDIT_BYTES,
  OUTPUT_CHUNK_BYTES,
  MAX_INPUT_BYTES,
  createOutputDelivery,
  createInputWriter,
} = require("../electron/terminal-flow.cjs");
const {
  createTerminalOutput,
  createTerminalInput,
} = require("../src/terminal-output.ts");
const { openHerdrStream } = require("../electron/terminal-stream.cjs");
const {
  herdrLaunchParams,
  createFrameDecoder,
} = require("../electron/terminal-text.cjs");
const { registerTerminalIpc } = require("../electron/ipc/terminals.cjs");

test("unacknowledged output stops at the byte credit and resumes without splitting Unicode or dropping ANSI", () => {
  const sent = [];
  const flow = createOutputDelivery({
    streamId: "new",
    send: (event) => sent.push(event),
    changed() {},
  });
  const text = "\x1b[31mПривет 🌍\x1b[0m".repeat(100000);
  flow.enqueue(text, true);
  assert.equal(flow.blocked, true);
  assert.ok(flow.stats.bytes <= OUTPUT_CREDIT_BYTES);
  assert.ok(
    sent.every((event) => Buffer.byteLength(event.data) <= OUTPUT_CHUNK_BYTES),
  );
  const before = sent.length;
  flow.ack("old", 1);
  assert.equal(sent.length, before);
  const all = [];
  while (sent.length) {
    const event = sent.shift();
    all.push(event);
    flow.ack(event.streamId, event.sequence);
    flow.ack(event.streamId, event.sequence);
    assert.ok(flow.stats.bytes <= OUTPUT_CREDIT_BYTES);
  }
  assert.equal(all.map((event) => event.data).join(""), text);
  assert.equal(all.filter((event) => event.reset).length, 1);
  assert.equal(flow.stats.bytes, 0);
});

test("initial attachment and selection withhold acknowledgements until xterm processed the data", () => {
  const callbacks = [];
  const written = [];
  const acks = [];
  let resets = 0;
  const renderer = createTerminalOutput({
    streamId: "new",
    write(data, done) {
      written.push(data);
      callbacks.push(done);
    },
    reset() {
      resets++;
    },
    ack(...args) {
      acks.push(args);
    },
    fail(message) {
      assert.fail(message);
    },
  });
  renderer.push({
    data: "old image",
    streamId: "old",
    sequence: 1,
    reset: true,
  });
  renderer.push({ data: "first", streamId: "new", sequence: 1, reset: true });
  renderer.push({ data: "second", streamId: "new", sequence: 2 });
  assert.deepEqual(written, []);
  renderer.start();
  assert.deepEqual(written, ["first"]);
  assert.equal(resets, 1);
  assert.deepEqual(acks, []);
  renderer.pause(true);
  callbacks.shift()();
  assert.deepEqual(acks, [["new", 1]]);
  assert.deepEqual(written, ["first"]);
  renderer.pause(false);
  callbacks.shift()();
  assert.deepEqual(written, ["first", "second"]);
  assert.deepEqual(acks, [
    ["new", 1],
    ["new", 2],
  ]);
  assert.equal(renderer.stats.bytes, 0);
  renderer.close();
  assert.equal(renderer.accepts("new"), false);
});

test("selection holds a fixed credit through repeated checks, then processes every character", () => {
  let renderer;
  const callbacks = [];
  let received = "";
  const flow = createOutputDelivery({
    streamId: "load",
    send: (event) => renderer.push(event),
    changed() {},
  });
  renderer = createTerminalOutput({
    streamId: "load",
    write(data, done) {
      received += data;
      callbacks.push(done);
    },
    reset() {},
    ack: (token, sequence) => flow.ack(token, sequence),
    fail: assert.fail,
  });
  renderer.start();
  renderer.pause(true);
  const frame = "\x1b[32mUnicode 🌍\x1b[0m".repeat(40000);
  flow.enqueue(frame, true);
  const held = renderer.stats.bytes;
  for (let seconds = 0; seconds < 600; seconds++) {
    assert.equal(renderer.stats.bytes, held);
    assert.ok(held <= OUTPUT_CREDIT_BYTES);
    assert.equal(flow.blocked, true);
  }
  renderer.pause(false);
  while (callbacks.length) callbacks.shift()();
  assert.equal(received, frame);
  assert.equal(flow.stats.bytes, 0);
});

test("worktree terminals receive the same explicit environment and UTF-8 locale as other creation paths", () => {
  for (const method of ["workspace.create", "pane.split", "worktree.create"]) {
    const params = herdrLaunchParams(method, {
      env: { PROJECT_ENV: "configured", LC_ALL: "C" },
    });
    assert.equal(params.env.PROJECT_ENV, "configured");
    assert.equal(params.env.LANG, "en_US.UTF-8");
    assert.equal(params.env.LC_ALL, "en_US.UTF-8");
  }
});

test("frame bounds count decoded bytes and reject noncanonical base64", () => {
  const decode = createFrameDecoder(4);
  assert.equal(decode(Buffer.from("four").toString("base64"), true), "four");
  assert.throws(
    () => decode(Buffer.from("fives").toString("base64")),
    /supported size/,
  );
  for (const value of [
    "ab==",
    "!!!!",
    "a===",
    "YWJjZA",
    "YWJjZA==\n",
    "YW=JjZA=",
  ])
    assert.throws(() => decode(value), /encoding/);
});

test("renderer reload attaches a fresh generation, ignores old acknowledgements, and retains the daemon pane", async () => {
  const handlers = new Map();
  const terminals = new Map();
  const opens = [];
  const acknowledgements = [];
  const killed = [];
  const ipc = registerTerminalIpc({
    handle: (name, handler) => handlers.set(name, handler),
    send() {},
    getConnections: () => ({}),
    executable: () => "herdr",
    id: (value) => {
      if (typeof value !== "string" || !value || value.length > 200)
        throw new Error("Invalid raw Herdr ID");
      return value;
    },
    terminals,
    terminalPending: new Map(),
    openStream: async (options) => {
      opens.push(options);
      return {
        history: "",
        source: "herdr",
        streamId: options.streamId,
        proc: { kill: () => killed.push(options.streamId) },
        ack: (...args) => acknowledgements.push(args),
      };
    },
  });
  try {
    const open = handlers.get("terminal-open");
    const options = {
      panelId: `herdr:v2:${encodeURIComponent(`/tmp/${String.fromCodePoint(0x044f).repeat(30)}.sock`)}:w1%3Ap1`,
      herdrId: "live-pane",
      endpoint: "/tmp/test-herdr.sock",
      streamId: "first",
    };
    assert.ok(options.panelId.length > 200);
    assert.equal((await open(options)).streamId, "first");
    assert.equal((await open(options)).streamId, "first");
    assert.equal(opens.length, 1);
    assert.equal(
      (await open({ ...options, streamId: "second" })).streamId,
      "second",
    );
    assert.equal(opens.length, 2);
    assert.deepEqual(killed, ["first"]);
    assert.equal(opens[1].target, "live-pane");
    handlers.get("terminal-ack")(options.panelId, "second", 1);
    assert.deepEqual(acknowledgements, [["second", 1]]);
    await assert.rejects(
      open({ ...options, panelId: "x".repeat(4097) }),
      /Invalid panel ID/,
    );
    await assert.rejects(
      open({ ...options, panelId: "new-panel", herdrId: "x".repeat(201) }),
      /Invalid raw Herdr ID/,
    );
    terminals.get(options.panelId).exited = true;
    await handlers.get("terminal-close")(options.panelId);
    assert.deepEqual(killed, ["first", "second"]);
    assert.equal(terminals.size, 0);
  } finally {
    ipc.close();
  }
});

test("replacement and close serialize delayed Herdr controller releases before opening another generation", async () => {
  const handlers = new Map();
  const releases = new Map();
  const log = [];
  let controller;
  const ipc = registerTerminalIpc({
    handle: (name, handler) => handlers.set(name, handler),
    send() {},
    getConnections: () => ({}),
    executable: () => "herdr",
    id: (value) => value,
    terminals: new Map(),
    terminalPending: new Map(),
    openStream: async ({ streamId }) => {
      assert.equal(
        controller,
        undefined,
        "Previous controller is still attached",
      );
      controller = streamId;
      log.push(`open ${streamId}`);
      let release;
      return {
        source: "herdr",
        history: "",
        exited: false,
        streamId,
        proc: {
          kill: () => {
            if (!release) {
              log.push(`release ${streamId}`);
              release = new Promise((resolve) => {
                releases.set(streamId, () => {
                  controller = undefined;
                  resolve();
                });
              });
            }
            return release;
          },
        },
      };
    },
  });
  const open = handlers.get("terminal-open");
  const options = {
    panelId: "same-panel",
    herdrId: "live-pane",
    endpoint: "/tmp/test-herdr.sock",
  };
  try {
    await open({ ...options, streamId: "first" });
    const second = open({ ...options, streamId: "second" });
    const third = open({ ...options, streamId: "third" });
    assert.deepEqual(log, ["open first", "release first"]);
    releases.get("first")();
    assert.equal((await second).exited, true);
    assert.equal((await third).streamId, "third");
    assert.deepEqual(log, ["open first", "release first", "open third"]);
    const close = handlers.get("terminal-close")("same-panel");
    const fourth = open({ ...options, streamId: "fourth" });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(controller, "third");
    assert.equal(log.at(-1), "release third");
    releases.get("third")();
    await close;
    assert.equal((await fourth).streamId, "fourth");
    assert.equal(controller, "fourth");
    const finalClose = handlers.get("terminal-close")("same-panel");
    await new Promise((resolve) => setImmediate(resolve));
    releases.get("fourth")();
    await finalClose;
  } finally {
    ipc.close();
  }
});

test("stdin respects drain, rejects overflow explicitly, and resolves only flushed input", async () => {
  const stdin = new EventEmitter();
  const calls = [];
  stdin.write = (line, callback) => {
    calls.push({ line, callback });
    return false;
  };
  const writer = createInputWriter(stdin);
  let firstDone = false;
  const first = writer.write({ bytes: "a".repeat(1000000) }).then(() => {
    firstDone = true;
  });
  const second = writer.write({ bytes: "b".repeat(1000000) });
  assert.equal(calls.length, 1);
  await assert.rejects(
    writer.write({ bytes: "c".repeat(100000) }),
    /queue is full/,
  );
  assert.ok(writer.stats.bytes <= MAX_INPUT_BYTES);
  assert.equal(firstDone, false);
  calls[0].callback();
  stdin.emit("drain");
  assert.equal(calls.length, 2);
  calls[1].callback();
  await Promise.all([first, second]);
  assert.equal(writer.stats.bytes, 0);
  assert.equal(JSON.parse(calls[0].line).bytes, "a".repeat(1000000));
  assert.equal(JSON.parse(calls[1].line).bytes, "b".repeat(1000000));
});

test("stdin disconnect rejects every queued write instead of reporting success", async () => {
  const stdin = new EventEmitter();
  stdin.write = () => false;
  const writer = createInputWriter(stdin);
  const first = assert.rejects(
    writer.write({ type: "terminal.input", bytes: "YQ==" }),
    /closed/,
  );
  const second = assert.rejects(
    writer.write({ type: "terminal.input", bytes: "Yg==" }),
    /closed/,
  );
  stdin.emit("close");
  await Promise.all([first, second]);
  assert.equal(writer.stats.bytes, 0);
});

test("renderer limits pending IPC input and preserves paste and Unicode order", async () => {
  const sent = [];
  const errors = [];
  const callbacks = [];
  const input = createTerminalInput(
    (data) => {
      sent.push(data);
      return new Promise((resolve) => callbacks.push(resolve));
    },
    (message) => errors.push(message),
  );
  input.send("Привет 🌍");
  input.send("paste".repeat(180000));
  input.send("overflow".repeat(50000));
  assert.equal(errors.length, 1);
  assert.ok(input.stats.bytes <= 1024 * 1024);
  callbacks.shift()();
  await new Promise((resolve) => setImmediate(resolve));
  callbacks.shift()();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, ["Привет 🌍", "paste".repeat(180000)]);
  assert.equal(input.stats.bytes, 0);
});

function fakeChild() {
  const child = new EventEmitter();
  child.stdin = new Writable({
    write(_chunk, _encoding, callback) {
      callback();
    },
  });
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.kill = () => {
    child.emit("close", 0);
    return true;
  };
  return child;
}

async function fakeStream(child, events) {
  return openHerdrStream({
    endpoint: "/tmp/test-herdr.sock",
    panelId: "test",
    target: "pane",
    cols: 80,
    rows: 24,
    connections: {
      socket: async () => {
        throw new Error("no process lookup");
      },
    },
    binary: "herdr",
    send: (_channel, event) => events.push(event),
    spawnProcess: () => child,
    preflight: async () => ({ cli: { binary: "herdr" } }),
  });
}

test("bad NDJSON, invalid base64 and oversized frames disconnect visibly without closing the daemon pane", async () => {
  for (const line of [
    "not JSON\n",
    JSON.stringify({ type: "terminal.frame", full: true, bytes: "!!!!" }) +
      "\n",
    JSON.stringify({
      type: "terminal.frame",
      full: true,
      bytes: "a".repeat(6 * 1024 * 1024),
    }) + "\n",
  ]) {
    const child = fakeChild();
    const events = [];
    const stream = await fakeStream(child, events);
    child.stdout.write(line);
    assert.equal(stream.exited, true);
    assert.ok(events.at(-1).error);
    assert.equal(events.at(-1).data, "");
    assert.equal(stream.flowStats().parserBytes, 0);
  }
});

test("full initial frame replaces the image, fragmented UTF-8 survives, and old acknowledgements cannot resume a new stream", async () => {
  const child = fakeChild();
  const events = [];
  const stream = await fakeStream(child, events);
  const frame = (bytes, full) =>
    JSON.stringify({
      type: "terminal.frame",
      bytes: bytes.toString("base64"),
      full,
    }) + "\n";
  child.stdout.write(frame(Buffer.from("fresh screen\x1b[31m"), true));
  const word = Buffer.from("Привет 🌍");
  for (const byte of word)
    child.stdout.write(frame(Buffer.from([byte]), false));
  assert.equal(
    events.map((event) => event.data).join(""),
    "fresh screen\x1b[31mПривет 🌍",
  );
  assert.equal(events[0].reset, true);
  child.stdout.write(frame(Buffer.alloc(500000, 65), false));
  assert.equal(child.stdout.isPaused(), true);
  const held = stream.flowStats().output.bytes;
  for (const event of events) stream.ack("old", event.sequence);
  assert.equal(stream.flowStats().output.bytes, held);
  let index = 0;
  while (index < events.length) {
    const event = events[index++];
    stream.ack(event.streamId, event.sequence);
  }
  assert.equal(stream.flowStats().output.bytes, 0);
  assert.equal(child.stdout.isPaused(), false);
  stream.proc.kill();
});

test("releasing a real CLI while ACK delivery is paused drains its pipe and completes detach", async () => {
  const child = spawn(
    process.execPath,
    [
      "-e",
      `
    process.stdout.write(JSON.stringify({ type: 'terminal.frame', full: true,
      bytes: Buffer.alloc(500000, 65).toString('base64') }) + String.fromCharCode(10));
    process.stdin.on('data', () => process.exit(0));
  `,
    ],
    { stdio: ["pipe", "pipe", "pipe"] },
  );
  const events = [];
  const stream = await fakeStream(child, events);
  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        clearInterval(check);
        reject(new Error("CLI never supplied its frame"));
      }, 1000);
      const check = setInterval(() => {
        if (!child.stdout.isPaused()) return;
        clearInterval(check);
        clearTimeout(timeout);
        resolve();
      }, 10);
      timeout.unref();
    });
    const closed = once(child, "close");
    stream.proc.kill();
    await Promise.race([
      closed,
      new Promise((_resolve, reject) => {
        const timeout = setTimeout(
          () => reject(new Error("Paused CLI pipe prevented detach")),
          1000,
        );
        timeout.unref();
      }),
    ]);
    assert.equal(stream.exited, true);
    assert.equal(events.at(-1).error, undefined);
    assert.equal(stream.flowStats().parserBytes, 0);
  } finally {
    child.stdout.destroy();
    child.kill();
  }
});

test(
  "corrupt output from a real CLI disconnects visibly without waiting on a paused pipe",
  { timeout: 2000 },
  async () => {
    const child = spawn(
      process.execPath,
      [
        "-e",
        `
    process.stdout.write('invalid NDJSON' + String.fromCharCode(10));
    setInterval(() => process.stdout.write('more invalid NDJSON' + String.fromCharCode(10)), 10);
  `,
      ],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    const events = [];
    const closed = once(child, "close");
    const stream = await fakeStream(child, events);
    try {
      await closed;
      assert.equal(stream.exited, true);
      assert.match(events.at(-1).error, /Reconnect/);
    } finally {
      child.stdout.destroy();
      child.kill();
    }
  },
);

test("a disconnected selected stream drains every valid final byte when acknowledged", async () => {
  const child = fakeChild();
  const events = [];
  const stream = await fakeStream(child, events);
  child.stdout.write(
    JSON.stringify({
      type: "terminal.frame",
      bytes: Buffer.alloc(500000, 65).toString("base64"),
      full: true,
    }) + "\n",
  );
  assert.equal(child.stdout.isPaused(), true);
  child.emit("exit", 13);
  assert.equal(stream.exited, true);
  assert.equal(child.stdout.destroyed, false);
  assert.equal(events.at(-1).exitCode, 13);
  assert.match(events.at(-1).error, /Reconnect/);
  await assert.rejects(stream.proc.write("input"), /disconnected/);
  const count = events.length;
  child.emit("close", 13);
  assert.equal(events.length, count);
  assert.equal(stream.exited, true);
  let index = 0;
  while (index < events.length) {
    const event = events[index++];
    if (event.sequence) stream.ack(event.streamId, event.sequence);
    assert.ok(stream.flowStats().output.bytes <= OUTPUT_CREDIT_BYTES);
  }
  assert.equal(events.map((event) => event.data).join(""), "A".repeat(500000));
  assert.equal(stream.flowStats().output.bytes, 0);
  assert.equal(child.stdout.isPaused(), false);
  stream.proc.kill();
});

test(
  "a real exited CLI drains remaining stdout after the selection releases",
  { timeout: 3000 },
  async () => {
    const child = spawn(
      process.execPath,
      [
        "-e",
        `
      const line = bytes => JSON.stringify({ type: 'terminal.frame', full: true,
        bytes: Buffer.from(bytes).toString('base64') }) + String.fromCharCode(10);
      process.stdout.write(line('A'.repeat(500000)));
      process.stdout.write(line('Привет 🌍'), () => process.exit(13));
    `,
      ],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    const events = [];
    const stream = await fakeStream(child, events);
    const closed = once(child, "close");
    try {
      await new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
          clearInterval(check);
          reject(new Error("CLI did not exit while its frame was paused"));
        }, 1000);
        const check = setInterval(() => {
          if (!stream.exited || !child.stdout.isPaused()) return;
          clearInterval(check);
          clearTimeout(timeout);
          resolve();
        }, 10);
      });
      let index = 0;
      await new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
          clearInterval(check);
          reject(new Error("Exited CLI output did not finish draining"));
        }, 1000);
        const check = setInterval(() => {
          while (index < events.length) {
            const event = events[index++];
            if (event.sequence) stream.ack(event.streamId, event.sequence);
          }
          if (!child.stdout.readableEnded || stream.flowStats().output.bytes)
            return;
          clearInterval(check);
          clearTimeout(timeout);
          resolve();
        }, 5);
      });
      await closed;
      assert.equal(
        events.map((event) => event.data).join(""),
        "A".repeat(500000) + "Привет 🌍",
      );
      assert.equal(stream.flowStats().parserBytes, 0);
      assert.equal(stream.flowStats().output.bytes, 0);
      assert.equal(
        events.filter((event) => event.exitCode !== undefined).length,
        1,
      );
    } finally {
      child.stdout.destroy();
      child.kill();
    }
  },
);
