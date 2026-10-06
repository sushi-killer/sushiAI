const test = require("node:test");
const assert = require("node:assert/strict");
const {
  createTerminalHandlers,
  FIRST_ATTACH_SCROLLBACK,
} = require("../electron/daemon/terminals.cjs");
const { OUTPUT_CREDIT_BYTES } = require("../electron/terminal-flow.cjs");

function setup(options = {}) {
  const sent = [];
  const requests = [];
  const attaches = [];
  let eventListener;
  let detached = 0;
  const manager = {
    async attach(host, sessionId, options, onBytes) {
      const attachment = { host, sessionId, options, onBytes };
      attaches.push(attachment);
      onBytes(Buffer.from("screen"), { snapshot: true });
      return {
        cols: 100,
        rows: 30,
        seq: 6,
        detach: async () => {
          detached++;
        },
      };
    },
    async request(host, method, params) {
      requests.push({ host, method, params });
    },
  };
  const handlers = createTerminalHandlers({
    ...options,
    getManager: () => manager,
    send: (channel, value) => sent.push({ channel, ...value }),
    onEvent: (callback) => {
      eventListener = callback;
      return () => {
        eventListener = undefined;
      };
    },
  });
  return {
    handlers,
    sent,
    requests,
    attaches,
    emit: (event) => eventListener?.(event),
    detached: () => detached,
  };
}
const attachInput = (extra = {}) => ({
  panelId: "p1",
  host: "local",
  sessionId: "s1",
  cols: 100,
  rows: 30,
  ...extra,
});

test("snapshot is sent before bytes with the attach size", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  t.attaches[0].onBytes(Buffer.from("hello"));
  assert.deepEqual(
    t.sent.map(({ channel, ...rest }) => rest),
    [
      { panelId: "p1", snapshot: "screen", cols: 100, rows: 30 },
      { panelId: "p1", data: "hello" },
    ],
  );
});

test("every attach asks for the history, because the terminal it feeds is empty", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  await t.handlers.attach(attachInput());
  await t.handlers.attach(attachInput({ panelId: "p2", sessionId: "s2" }));
  await t.handlers.attach(attachInput({ panelId: "p3", sessionId: "s2" }));
  assert.deepEqual(
    t.attaches.map((a) => a.options.scrollback),
    Array(4).fill(FIRST_ATTACH_SCROLLBACK),
  );
  assert.equal(FIRST_ATTACH_SCROLLBACK, 2000);
});

test("attach resizes the session when the panel size differs", async () => {
  const t = setup();
  await t.handlers.attach(attachInput({ cols: 120, rows: 40 }));
  assert.deepEqual(t.requests, [
    {
      host: "local",
      method: "session.resize",
      params: { id: "s1", cols: 120, rows: 40 },
    },
  ]);
});

test("write, resize and attach-file map to session requests", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  await t.handlers.write("p1", "ls\r");
  await t.handlers.resize("p1", 90, 20);
  await t.handlers.attachFile("p1", "/tmp/it's.png");
  assert.deepEqual(t.requests, [
    {
      host: "local",
      method: "session.input",
      params: { id: "s1", data: "ls\r" },
    },
    {
      host: "local",
      method: "session.resize",
      params: { id: "s1", cols: 90, rows: 20 },
    },
    {
      host: "local",
      method: "session.input",
      params: { id: "s1", data: `'/tmp/it'\\''s.png' ` },
    },
  ]);
  await assert.rejects(t.handlers.write("missing", "x"), /not attached/);
});

test("detach releases the handle and stops delivery", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  const { onBytes } = t.attaches[0];
  await t.handlers.detach("p1");
  assert.equal(t.detached(), 1);
  const before = t.sent.length;
  onBytes(Buffer.from("late"));
  assert.equal(t.sent.length, before);
  await t.handlers.detach("p1");
  assert.equal(t.detached(), 1);
});

test("exited event reaches only the matching panel", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  await t.handlers.attach(attachInput({ panelId: "p2", sessionId: "s2" }));
  t.emit({ host: "other", method: "session.exited", params: { id: "s1" } });
  t.emit({ host: "local", method: "session.output", params: { id: "s1" } });
  t.emit({ host: "local", method: "session.exited", params: { id: "s1" } });
  assert.deepEqual(
    t.sent.filter((m) => m.exited).map(({ channel, ...rest }) => rest),
    [{ panelId: "p1", exited: true }],
  );
});

test("a resync snapshot drops queued bytes and resets the credit", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  const { onBytes } = t.attaches[0];
  onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES * 2, "a")); // fills the credit
  const dataBefore = t.sent.filter((m) => m.data !== undefined);
  assert.ok(dataBefore.length >= 1);
  onBytes(Buffer.from("fresh"), { snapshot: true, cols: 80, rows: 24 });
  const last = t.sent.at(-1);
  assert.deepEqual(
    { snapshot: last.snapshot, cols: last.cols, rows: last.rows },
    { snapshot: "fresh", cols: 80, rows: 24 },
  );
  const count = t.sent.length;
  t.handlers.ack("p1", 5); // old queue is gone, nothing more is released
  assert.equal(t.sent.length, count);
  onBytes(Buffer.from("next"));
  assert.equal(t.sent.at(-1).data, "next");
});

test("output is paced by acknowledged bytes and stays in order", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  t.handlers.ack("p1", 6); // the snapshot is written
  const total = OUTPUT_CREDIT_BYTES * 3;
  t.attaches[0].onBytes(Buffer.alloc(total, "b"));
  const sentBytes = () =>
    t.sent.filter((m) => m.data).reduce((n, m) => n + m.data.length, 0);
  assert.equal(sentBytes(), OUTPUT_CREDIT_BYTES);
  while (sentBytes() < total) {
    const before = sentBytes();
    t.handlers.ack("p1", OUTPUT_CREDIT_BYTES);
    assert.ok(sentBytes() > before, "an ack releases more output");
    assert.ok(sentBytes() - before <= OUTPUT_CREDIT_BYTES);
  }
  assert.equal(sentBytes(), total);
});

test("a multi-byte character split across chunks is rejoined", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  const bytes = Buffer.from("é");
  t.attaches[0].onBytes(bytes.subarray(0, 1));
  t.attaches[0].onBytes(bytes.subarray(1));
  assert.deepEqual(
    t.sent.filter((m) => m.data).map((m) => m.data),
    ["é"],
  );
});

test("a failed attach leaves no panel behind", async () => {
  const handlers = createTerminalHandlers({
    getManager: () => ({
      attach: async () => {
        throw new Error("host not ready");
      },
      request: async () => {},
    }),
    send: () => {},
  });
  await assert.rejects(handlers.attach(attachInput()), /host not ready/);
  await assert.rejects(handlers.write("p1", "x"), /not attached/);
});

test("daemon IPC channels dispatch to the handlers after validation", async () => {
  const { registerDaemonIpc } = require("../electron/ipc/daemon.cjs");
  const t = setup();
  const channels = new Map();
  registerDaemonIpc({
    handle: (channel, run) => channels.set(channel, run),
    send: (channel, value) => t.sent.push({ channel, ...value }),
    getManager: () => ({
      attach: async (...args) => {
        t.attaches.push(args);
        return { cols: 80, rows: 24, seq: 0, detach: async () => {} };
      },
      request: async (host, method, params) =>
        t.requests.push({ host, method, params }),
    }),
  });
  await channels.get("daemon-terminal-attach")(
    attachInput({ cols: 80, rows: 24 }),
  );
  await channels.get("daemon-terminal-write")("p1", "x");
  assert.equal(t.requests[0].method, "session.input");
  await assert.rejects(
    channels.get("daemon-terminal-write")("p1", 5),
    /Invalid data/,
  );
});

test("pasted data is stored privately on this Mac and its path is typed", async () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "attach-data-"));
  const attachmentsDir = path.join(dir, "attachments");
  try {
    const t = setup({ attachmentsDir });
    await t.handlers.attach(attachInput());
    await t.handlers.attachData("p1", "../shot 1.png", Buffer.from("png"));
    const [write] = t.requests;
    assert.equal(write.method, "session.input");
    const typed = write.params.data.trim().slice(1, -1);
    assert.equal(path.dirname(typed), attachmentsDir);
    assert.match(path.basename(typed), /^[0-9a-f-]{36}-shot 1\.png$/);
    assert.equal(fs.readFileSync(typed, "utf8"), "png");
    assert.equal(fs.statSync(typed).mode & 0o777, 0o600);
    assert.equal(fs.statSync(attachmentsDir).mode & 0o777, 0o700);
    await assert.rejects(
      t.handlers.attachData("p1", "empty.png", Buffer.alloc(0)),
      /non-empty/,
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("pasted data for a remote host is uploaded over exec and its remote path typed", async () => {
  const calls = [];
  const t = setup({
    exec: async (endpoint, command, options) => {
      calls.push({ endpoint, command, options });
      return "/home/dev/.sushiai/attachments/abc-a.png\n";
    },
  });
  await t.handlers.attach(attachInput({ host: "host-1" }));
  await t.handlers.attachData("p1", "../a b.png", Buffer.from("xyz"));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].endpoint, "ssh:host-1");
  assert.equal(Buffer.from(calls[0].options.input).toString(), "xyz");
  assert.match(calls[0].command, /^sh -c '/);
  assert.match(calls[0].command, /umask 077/);
  assert.match(calls[0].command, /\.sushiai\/attachments/);
  assert.match(calls[0].command, /chmod 700/);
  assert.match(calls[0].command, /chmod 600/);
  assert.match(calls[0].command, /[0-9a-f-]{36}-a b\.png/);
  assert.equal(
    t.requests[0].params.data,
    "'/home/dev/.sushiai/attachments/abc-a.png' ",
  );
  await assert.rejects(
    t.handlers.attachData("p1", "big.bin", Buffer.alloc(20 * 1024 * 1024 + 1)),
    /20 MB/,
  );
  assert.equal(calls.length, 1);
  await assert.rejects(
    t.handlers.attachData("missing", "a.png", Buffer.from("x")),
    /not attached/,
  );
});

test("a host that does not return an absolute path is an upload failure", async () => {
  const t = setup({ exec: async () => "oops" });
  await t.handlers.attach(attachInput({ host: "host-1" }));
  await assert.rejects(
    t.handlers.attachData("p1", "a.png", Buffer.from("x")),
    /did not confirm/,
  );
});

test("a surrogate pair that does not fit the credit waits for an ack instead of spinning", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  t.handlers.ack("p1", 6);
  const { onBytes } = t.attaches[0];
  onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES - 2, "a"));
  onBytes(Buffer.from("\u{1F680}xyz"));
  const text = () =>
    t.sent
      .filter((m) => m.data)
      .map((m) => m.data)
      .join("");
  assert.equal(text().length, OUTPUT_CREDIT_BYTES - 2);
  t.handlers.ack("p1", OUTPUT_CREDIT_BYTES);
  assert.ok(text().endsWith("\u{1F680}xyz"));
  assert.equal(text().length, OUTPUT_CREDIT_BYTES - 2 + 5);
});

test("acks of bytes sent before a snapshot do not shrink the new window", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  const { onBytes } = t.attaches[0];
  t.handlers.ack("p1", 6);
  onBytes(Buffer.alloc(1000, "a"));
  onBytes(Buffer.from("screen"), { snapshot: true });
  t.handlers.ack("p1", 1000); // the old 1000 bytes
  onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES, "b"));
  const sent = t.sent
    .filter((m) => m.data)
    .reduce((n, m) => n + m.data.length, 0);
  // The snapshot's 6 bytes are still unacked: the window is CREDIT - 6.
  assert.equal(sent, 1000 + OUTPUT_CREDIT_BYTES - 6);
});

test("a renderer that stops acking cannot grow the queue: it is dropped and a fresh snapshot taken", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  const { onBytes } = t.attaches[0];
  for (let i = 0; i < 6; i++) onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES, "c"));
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(t.attaches.length, 2);
  assert.equal(t.attaches[1].options.scrollback, FIRST_ATTACH_SCROLLBACK);
  assert.equal(t.detached(), 1);
  assert.equal(t.sent.filter((m) => m.snapshot !== undefined).length, 2);
});

test("a dropped file on a remote host is uploaded, not typed by its Mac path", async () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "drop-"));
  try {
    const file = path.join(dir, "shot.png");
    fs.writeFileSync(file, "png-bytes");
    const calls = [];
    const t = setup({
      exec: async (endpoint, command, options) => {
        calls.push({ endpoint, command, options });
        return "/home/dev/.sushiai/attachments/u-shot.png\n";
      },
    });
    await t.handlers.attach(attachInput({ host: "host-1" }));
    await t.handlers.attachFile("p1", file);
    assert.equal(calls.length, 1);
    assert.equal(Buffer.from(calls[0].options.input).toString(), "png-bytes");
    assert.match(calls[0].command, /shot\.png/);
    const typed = t.requests.at(-1).params.data;
    assert.equal(typed, "'/home/dev/.sushiai/attachments/u-shot.png' ");
    assert.ok(!typed.includes(dir));
    // The cap applies; a directory is no file.
    const big = path.join(dir, "big.bin");
    fs.writeFileSync(big, Buffer.alloc(20 * 1024 * 1024 + 1));
    await assert.rejects(t.handlers.attachFile("p1", big), /20 MB/);
    await assert.rejects(t.handlers.attachFile("p1", dir), /Choose/);
    assert.equal(calls.length, 1);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("a re-attach for the same panel carries the unacked bytes into the stale window", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  t.handlers.ack("p1", 6);
  t.attaches[0].onBytes(Buffer.alloc(1000, "a"));
  // 1000 bytes are out and unacked; the panel attaches again.
  await t.handlers.attach(attachInput());
  const { onBytes } = t.attaches[1];
  t.handlers.ack("p1", 1000); // the old window's bytes arrive late
  t.handlers.ack("p1", 6); // the new snapshot is written
  onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES, "b"));
  const sent = t.sent
    .filter((m) => m.data)
    .map((m) => m.data.length)
    .reduce((n, size) => n + size, 0);
  // Full credit for the new window: the late acks did not eat into it.
  assert.equal(sent, 1000 + OUTPUT_CREDIT_BYTES);
});

test("a failed overflow re-attach tells the renderer the terminal ended", async () => {
  const sent = [];
  let attaches = 0;
  let onBytes;
  const handlers = createTerminalHandlers({
    getManager: () => ({
      attach: async (host, id, options, callback) => {
        attaches++;
        if (attaches > 1) throw new Error("host not ready");
        onBytes = callback;
        callback(Buffer.from("screen"), { snapshot: true });
        return { cols: 80, rows: 24, seq: 0, detach: async () => {} };
      },
      request: async () => {},
    }),
    send: (channel, value) => sent.push(value),
  });
  await handlers.attach(attachInput({ cols: 80, rows: 24 }));
  for (let i = 0; i < 6; i++) onBytes(Buffer.alloc(OUTPUT_CREDIT_BYTES, "c"));
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(sent.at(-1), { panelId: "p1", exited: true });
});

test("closing a host ends and detaches only its terminals", async () => {
  const t = setup();
  await t.handlers.attach(attachInput({ panelId: "p1", host: "host-1" }));
  await t.handlers.attach(attachInput({ panelId: "p2", host: "host-2" }));
  await t.handlers.closeHost("host-1");
  assert.deepEqual(
    t.sent
      .filter((m) => m.exited)
      .map(({ panelId, exited }) => ({ panelId, exited })),
    [{ panelId: "p1", exited: true }],
  );
  assert.equal(t.detached(), 1);
  await assert.rejects(t.handlers.write("p1", "x"), /not attached/);
  await t.handlers.write("p2", "x");
});
