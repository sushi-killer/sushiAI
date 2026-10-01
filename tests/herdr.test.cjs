const { test } = require("node:test");
const assert = require("node:assert/strict");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const { request, inputCommands } = require("../electron/herdr.cjs");

async function fixture(t, respond) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "bridge-rpc-"));
  const socketPath = path.join(directory, "api.sock");
  const connections = new Set();
  const server = net.createServer((socket) => {
    connections.add(socket);
    socket.on("close", () => connections.delete(socket));
    let buffer = "";
    socket.on("data", (data) => {
      buffer += data;
      if (buffer.includes("\n")) respond(socket, JSON.parse(buffer.trim()));
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    connections.forEach((socket) => socket.destroy());
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  });
  return socketPath;
}
test("correlates request IDs and decodes fragmented Unicode responses", async (t) => {
  const socket = await fixture(t, (client, message) => {
    assert.equal(message.method, "session.snapshot");
    client.write(JSON.stringify({ id: "other-request", result: {} }) + "\n");
    const data = Buffer.from(
      JSON.stringify({ id: message.id, result: { label: "Workspace 🍣" } }) +
        "\n",
    );
    const boundary = data.indexOf(Buffer.from("🍣")) + 1;
    client.write(data.subarray(0, boundary));
    setTimeout(() => client.write(data.subarray(boundary)), 10);
  });
  assert.deepEqual(await request(socket, "session.snapshot"), {
    label: "Workspace 🍣",
  });
});
test("propagates backend errors without resolving successfully", async (t) => {
  const socket = await fixture(t, (client, message) =>
    client.write(
      JSON.stringify({
        id: message.id,
        error: { code: "not_found", message: "Pane not found" },
      }) + "\n",
    ),
  );
  await assert.rejects(request(socket, "pane.read"), /Pane not found/);
  await assert.rejects(
    request(socket, "pane.read"),
    (error) => error.code === "not_found",
  );
});
test("Herdr polls cannot import the workspace used during a protected pane close", async (t) => {
  const {
    registerHerdrExtension,
  } = require("../electron/extensions/builtin-herdr.cjs");
  const calls = [];
  const socketPath = await fixture(t, (client, message) => {
    calls.push(message.method);
    const respond = (body) =>
      client.write(JSON.stringify({ id: message.id, ...body }) + "\n");
    if (message.method === "pane.close" && message.params.pane_id === "w1:p1")
      respond({
        error: {
          code: "confirmation_required",
          message: "closing this pane would close a worktree group",
        },
      });
    else if (message.method === "pane.move")
      setTimeout(
        () =>
          respond({
            result: {
              move_result: { changed: true, pane: { pane_id: "w3:p1" } },
            },
          }),
        25,
      );
    else if (message.method === "pane.close")
      setTimeout(() => respond({ result: { type: "ok" } }), 25);
    else
      respond({
        result: { snapshot: { workspaces: [], panes: [{ pane_id: "w2:p1" }] } },
      });
  });
  const handlers = new Map();
  registerHerdrExtension({
    handle: (channel, handler) => handlers.set(channel, handler),
    getConnections: () => ({ socket: async () => socketPath }),
    id: (value) => value,
  });
  const herdr = handlers.get("herdr");
  const [, snapshot] = await Promise.all([
    herdr("local", "pane.close", { pane_id: "w1:p1" }),
    herdr("local", "session.snapshot"),
  ]);
  assert.deepEqual(calls, [
    "session.snapshot",
    "pane.close",
    "pane.move",
    "pane.close",
    "session.snapshot",
  ]);
  assert.deepEqual(snapshot.snapshot.panes, [{ pane_id: "w2:p1" }]);
});

test("times out unresponsive sockets and rejects early disconnects", async (t) => {
  const socket = await fixture(t, () => {});
  await assert.rejects(request(socket, "ping", {}, 40), /did not respond/);
  const disconnected = await fixture(t, (client) => client.end());
  await assert.rejects(request(disconnected, "ping"), /disconnected/);
});
test("rejects malformed JSON instead of leaving requests pending", async (t) => {
  const socket = await fixture(t, (client) => client.write("invalid\n"));
  await assert.rejects(request(socket, "ping"), /Invalid JSON/);
});
test("preserves text and ordering around navigation, enter and control keys", () => {
  assert.deepEqual(inputCommands("hello 🍣\x1b[D\x7f!\r\x03"), [
    { text: "hello 🍣" },
    { keys: ["Left"] },
    { keys: ["Backspace"] },
    { text: "!" },
    { keys: ["Enter"] },
    { keys: ["Ctrl+c"] },
  ]);
});
test("multiline bracketed paste remains literal rather than executing lines", () => {
  assert.deepEqual(inputCommands("\x1b[200~echo one\necho two\x1b[201~"), [
    { text: "echo one\necho two" },
  ]);
});

test("herdr pane input validates the pane id before it reaches the socket", async () => {
  const {
    registerHerdrExtension,
  } = require("../electron/extensions/builtin-herdr.cjs");
  // The same validator main.cjs injects; it is the trust boundary for pane ids.
  const id = (value) => {
    if (typeof value !== "string" || !value || value.length > 200)
      throw new Error("Invalid panel ID.");
    return value;
  };
  const handlers = new Map();
  registerHerdrExtension({
    handle: (channel, callback) => handlers.set(channel, callback),
    getConnections: () => ({ socket: async () => "/tmp/herdr-test.sock" }),
    id,
  });
  const herdr = handlers.get("herdr");
  for (const paneId of [undefined, "", 123, "x".repeat(201)])
    await assert.rejects(
      herdr("ssh:demo", "pane.send_input", { pane_id: paneId, raw: "hi" }),
      /Invalid panel ID/,
      `pane_id ${JSON.stringify(paneId)} is refused`,
    );
});

test("a pane or workspace Herdr no longer has: closing it is fine, anything else says so plainly", async (t) => {
  const net = require("node:net");
  const os = require("node:os");
  const fs = require("node:fs/promises");
  const path = require("node:path");
  const {
    registerHerdrExtension,
    GONE_MESSAGE,
  } = require("../electron/extensions/builtin-herdr.cjs");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-gone-"));
  const socketPath = path.join(dir, "h.sock");
  const server = net.createServer((socket) => {
    socket.on("data", (chunk) => {
      const { id, method } = JSON.parse(String(chunk).split("\n")[0]);
      const what = method.startsWith("pane") ? "pane p9" : "workspace w5";
      socket.write(
        JSON.stringify({ id, error: { message: `${what} not found` } }) + "\n",
      );
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(dir, { recursive: true, force: true });
  });
  let call;
  registerHerdrExtension({
    handle: (_channel, callback) => (call = callback),
    getConnections: () => ({ socket: async () => socketPath }),
    id: (value) => String(value),
  });
  // Closing what is already closed is closed.
  assert.deepEqual(
    await call("local", "workspace.close", { workspace_id: "w5" }),
    {
      gone: true,
    },
  );
  assert.deepEqual(await call("local", "pane.close", { pane_id: "p9" }), {
    gone: true,
  });
  // Anything else gets a plain sentence, never the raw "wN not found".
  for (const [method, params] of [
    ["pane.send_input", { pane_id: "p9", text: "x", keys: [] }],
    ["pane.split", { workspace_id: "w5" }],
  ])
    await assert.rejects(call("local", method, params), (error) => {
      assert.equal(error.message, GONE_MESSAGE);
      return true;
    });
});

test("a session for a project on a host reuses its workspace, and an IPC wrapper is stripped", async () => {
  const { findHostWorkspace } =
    await import("../src/workspace/workspace-actions.ts");
  const { errorText, isGone } = await import("../src/app/errors.ts");
  const ws = (id, connection, cwd, herdrId = "h") => ({
    id,
    connection,
    cwd,
    herdrId,
    panels: [{ herdrId: "live" }],
  });
  const list = [
    ws("a", "ssh:lab", "/srv/app"),
    ws("b", "ssh:devbox", "/srv/app"),
    ws("c", "ssh:lab", "/srv/other"),
  ];
  // Same host and path: the same workspace, every time.
  assert.equal(findHostWorkspace(list, "local", "ssh:lab", "/srv/app").id, "a");
  // Another host has its own workspace; a new path has none yet.
  assert.equal(
    findHostWorkspace(list, "local", "ssh:devbox", "/srv/app").id,
    "b",
  );
  assert.equal(
    findHostWorkspace(list, "local", "ssh:lab", "/srv/new"),
    undefined,
  );
  // A workspace Herdr no longer lists (all its panes ended) is not reused.
  const gone = ws("g", "ssh:lab", "/srv/gone");
  gone.panels = [{ herdrId: "p", ended: true }];
  assert.equal(
    findHostWorkspace([gone], "local", "ssh:lab", "/srv/gone"),
    undefined,
  );
  // One just made is found by id before the host's listing shows its path.
  assert.equal(
    findHostWorkspace(list, "local", "ssh:lab", "/srv/new", "c").id,
    "c",
  );
  const wrapped = new Error(
    "Error invoking remote method 'herdr': Error: That session is no longer open on the host.",
  );
  assert.equal(
    errorText(wrapped),
    "That session is no longer open on the host.",
  );
  assert.equal(isGone(wrapped), true);
  assert.equal(isGone(new Error("boom")), false);
});
