const test = require("node:test");
const assert = require("node:assert/strict");
const {
  createTerminalHandlers,
  FIRST_ATTACH_SCROLLBACK,
} = require("../electron/daemon/terminals.cjs");
const { OUTPUT_CREDIT_BYTES } = require("../electron/terminal-flow.cjs");

function setup() {
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

test("scrollback is requested on the first attach of a session only", async () => {
  const t = setup();
  await t.handlers.attach(attachInput());
  await t.handlers.attach(attachInput());
  await t.handlers.attach(attachInput({ panelId: "p2", sessionId: "s2" }));
  await t.handlers.attach(attachInput({ panelId: "p3", sessionId: "s2" }));
  assert.deepEqual(
    t.attaches.map((a) => a.options.scrollback),
    [FIRST_ATTACH_SCROLLBACK, 0, FIRST_ATTACH_SCROLLBACK, 0],
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
