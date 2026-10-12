const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { connectDaemon } = require("../electron/daemon/client.cjs");
const { createTerminalHandlers } = require("../electron/daemon/terminals.cjs");

const { realDaemon } = require("./helpers/real-daemon.cjs");
const {
  DEADLINE_MS,
  until,
  pidStart,
  stopPid,
  removeDir,
} = require("./helpers/deadline.cjs");
const { binary, skip } = realDaemon();
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test(
  "terminals stream a real shell session through the daemon",
  { skip },
  async () => {
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "st-"));
    fs.chmodSync(home, 0o700);
    const daemon = spawn(binary, ["daemon"], {
      env: {
        PATH: process.env.PATH,
        SUSHIAI_HOME: home,
        HOME: home,
        CODEX_HOME: path.join(home, "codex"),
        SUSHIAI_LOG: "off",
      },
      stdio: "ignore",
    });
    const exited = new Promise((resolve) => daemon.once("exit", resolve));
    const timeout = { timeoutMs: DEADLINE_MS };
    const holders = new Map();
    let client;
    try {
      const socketPath = path.join(home, "daemon.sock");
      client = await until(async () => {
        if (!fs.existsSync(socketPath)) return undefined;
        return connectDaemon({
          socketPath,
          clientName: "terminals-test",
          helloTimeoutMs: DEADLINE_MS,
        }).catch(() => undefined);
      }, "the daemon socket");
      // Minimal manager: one host, no reconnect, no scrollback option.
      const manager = {
        request: (_host, method, params) =>
          client.request(method, params, timeout),
        attach: (_host, id, _options, onBytes) => client.attach(id, onBytes),
      };
      const sent = [];
      const handlers = createTerminalHandlers({
        getManager: () => manager,
        send: (channel, value) => {
          assert.equal(channel, "daemon-terminal-data");
          sent.push(value);
          const text = value.snapshot ?? value.data;
          if (text !== undefined)
            handlers.ack(value.panelId, Buffer.byteLength(text));
        },
      });
      const created = await client.request(
        "session.create",
        {
          cmd: ["/bin/sh"],
          cwd: home,
          cols: 80,
          rows: 24,
          idempotencyKey: "terminals-real-1",
        },
        timeout,
      );
      const id = created.id;
      for (const entry of await client.request("session.list", {}, timeout))
        if (entry.holderPid)
          holders.set(entry.holderPid, pidStart(entry.holderPid));
      assert.ok(id, "session id");
      const text = () => sent.map((m) => m.snapshot ?? m.data ?? "").join("");

      await handlers.attach({
        panelId: "panel-1",
        host: "local",
        sessionId: id,
        cols: 80,
        rows: 24,
      });
      assert.notEqual(sent[0].snapshot, undefined, "snapshot comes first");
      assert.equal(sent[0].cols, 80);

      await handlers.write("panel-1", "printf 'mark%s\\n' er-1\r");
      await until(() => text().includes("marker-1"), "marker-1");

      await handlers.resize("panel-1", 100, 30);
      await handlers.write("panel-1", "stty size\r");
      await until(() => text().includes("30 100"), "resized stty size");

      await handlers.detach("panel-1");
      const count = sent.length;
      await client.request(
        "session.input",
        { id, data: "echo late\r" },
        timeout,
      );
      // Gives a forbidden delivery after detach time to happen.
      await wait(200);
      assert.equal(sent.length, count, "nothing is delivered after detach");

      await client.request("session.close", { id, graceful: false }, timeout);
    } finally {
      // One ordered step: close the client, stop the daemon and the session
      // holders seen, wait for their exit, then remove the home.
      client?.close();
      await stopPid(daemon.pid);
      for (const [holder, start] of holders) await stopPid(holder, { start });
      await exited;
      removeDir(home);
    }
  },
);
