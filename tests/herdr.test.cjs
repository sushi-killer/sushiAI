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
