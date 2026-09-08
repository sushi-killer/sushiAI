const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const { spawnSync } = require("node:child_process");
const {
  storeTerminalAttachment,
} = require("../electron/terminal-attachments.cjs");

test("local Herdr attachments are readable, unique and private", async () => {
  const dataDir = await fs.mkdtemp("/tmp/sushiai-attachments-");
  try {
    const options = {
      terminal: { source: "pty", proc: { process: "claude" } },
      dataDir,
      name: "../../image.png",
      data: Buffer.from("image bytes").toString("base64"),
    };
    const [a, b] = await Promise.all([
      storeTerminalAttachment(options),
      storeTerminalAttachment(options),
    ]);
    assert.notEqual(a, b);
    assert.ok(a.startsWith(dataDir + "/attachments/"));
    assert.equal(await fs.readFile(a, "utf8"), "image bytes");
    assert.equal((await fs.stat(a)).mode & 0o777, 0o600);
    await assert.rejects(
      storeTerminalAttachment({ ...options, terminal: { exited: true } }),
    );
  } finally {
    await fs.rm(dataDir, { recursive: true, force: true });
  }
});

test("SSH Herdr uploads run the remote writer on the selected endpoint and return its path", async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-remote-upload-");
  try {
    const result = await storeTerminalAttachment({
      terminal: {
        source: "pty",
        remote: true,
        command: "claude",
        endpoint: "ssh:test-endpoint",
      },
      name: "my image.png",
      data: Buffer.from([137, 80, 78, 71]).toString("base64"),
      connections: {
        inspect: async (endpoint, options) => {
          assert.equal(endpoint, "ssh:test-endpoint");
          const child = spawnSync(
            "/usr/bin/python3",
            ["electron/remote-files.py"],
            {
              input: JSON.stringify(options),
              encoding: "utf8",
              env: { ...process.env, HOME: home },
            },
          );
          assert.equal(child.status, 0, child.stderr);
          const envelope = JSON.parse(child.stdout);
          assert.equal(envelope.error, undefined);
          return envelope.result;
        },
      },
    });
    assert.ok(result.startsWith(home + "/.cache/sushiai/attachments/"));
    assert.deepEqual(await fs.readFile(result), Buffer.from([137, 80, 78, 71]));
    assert.equal((await fs.stat(result)).mode & 0o777, 0o600);
  } finally {
    await fs.rm(home, { recursive: true, force: true });
  }
});

test("zsh and stale agent titles cannot upload files", async () => {
  for (const terminal of [
    {
      source: "pty",
      proc: { process: "zsh" },
      title: "Claude Code",
      lastAgent: "claude",
    },
    { source: "pty", remote: true },
  ]) {
    await assert.rejects(
      storeTerminalAttachment({
        terminal,
        name: "image.png",
        data: Buffer.from("image").toString("base64"),
      }),
      /active agent session/,
    );
  }
});

test("Herdr attachments check the live pane process, including returning to zsh", async (t) => {
  const net = require("node:net");
  const directory = await fs.mkdtemp("/tmp/attach-route-");
  const socketPath = directory + "/api.sock";
  let foreground = "claude";
  const server = net.createServer((socket) => {
    socket.once("data", (data) => {
      const message = JSON.parse(data);
      assert.equal(message.method, "pane.process_info");
      assert.equal(message.params.pane_id, "target-pane");
      socket.end(
        JSON.stringify({
          id: message.id,
          result: {
            process_info: { foreground_processes: [{ name: foreground }] },
          },
        }) + "\n",
      );
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  });
  const options = {
    terminal: { source: "herdr", endpoint: socketPath, target: "target-pane" },
    connections: { socket: async (endpoint) => endpoint },
    name: "image.png",
    data: Buffer.from("image").toString("base64"),
    dataDir: directory,
  };
  assert.equal(
    await fs.readFile(await storeTerminalAttachment(options), "utf8"),
    "image",
  );
  foreground = "zsh";
  await assert.rejects(
    storeTerminalAttachment(options),
    /active agent session/,
  );
});
