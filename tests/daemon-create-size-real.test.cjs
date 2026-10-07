const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { connectDaemon } = require("../electron/daemon/client.cjs");
const { createTerminalHandlers } = require("../electron/daemon/terminals.cjs");
const { createSessionLauncher } = require("../electron/session-launch.cjs");
const { toDaemonLaunch } = require("../src/workspace/session-launch.ts");
const { rememberTerminalSize } = require("../src/terminal-sizing.ts");

const binary = path.join(
  process.env.CARGO_TARGET_DIR || path.join(__dirname, "../target"),
  "debug/sushiai",
);
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check, what, ms = 10000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await wait(25);
  }
}

test(
  "a launch from a measured 40x9 pane starts the session at 40x9 and never resizes it",
  { skip: !fs.existsSync(binary) && "debug sushiai binary is not built" },
  async () => {
    // Short path under /tmp: unix socket paths are limited on macOS.
    const home = fs.mkdtempSync("/tmp/cs-");
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
    let client;
    try {
      const socketPath = path.join(home, "daemon.sock");
      client = await until(async () => {
        if (!fs.existsSync(socketPath)) return undefined;
        return connectDaemon({ socketPath, clientName: "size-test" }).catch(
          () => undefined,
        );
      }, "the daemon socket");
      const methods = [];
      const manager = {
        request: (_host, method, params) => {
          methods.push(method);
          return client.request(method, params);
        },
        attach: (_host, id, _options, onBytes) => client.attach(id, onBytes),
      };
      const launcher = createSessionLauncher({
        manager,
        environment: async () => ({
          env: { PATH: process.env.PATH, HOME: home },
          claudeSettings: {},
        }),
      });
      rememberTerminalSize({ cols: 40, rows: 9 });
      const request = toDaemonLaunch({
        kind: "terminal",
        endpoint: "local",
        cwd: home,
        label: "Shell",
        operationId: "size-1",
      });
      assert.equal(request.cols, 40);
      assert.equal(request.rows, 9);
      const { sessionId } = await launcher.launch(request);

      const sent = [];
      const handlers = createTerminalHandlers({
        getManager: () => manager,
        send: (_channel, value) => {
          sent.push(value);
          const text = value.snapshot ?? value.data;
          if (text !== undefined)
            handlers.ack(value.panelId, Buffer.byteLength(text));
        },
      });
      await handlers.attach({
        panelId: "p1",
        host: "local",
        sessionId,
        cols: 40,
        rows: 9,
      });
      assert.equal(sent[0].cols, 40);
      assert.equal(sent[0].rows, 9);
      await handlers.write("p1", "stty size; echo done\r");
      await until(
        () => sent.some((m) => (m.data ?? "").includes("done")),
        "stty output",
      );
      const text = sent.map((m) => m.snapshot ?? m.data ?? "").join("");
      assert.match(text, /9 40/);
      assert.ok(!methods.includes("session.resize"), "no resize was sent");
      await client.request("session.close", { id: sessionId, graceful: false });
    } finally {
      await client?.request("daemon.shutdown", {}).catch(() => {});
      client?.close();
      const stopped = await Promise.race([
        exited,
        wait(5000).then(() => false),
      ]);
      if (stopped === false) {
        daemon.kill("SIGTERM");
        await exited;
      }
      fs.rmSync(home, { recursive: true, force: true });
    }
  },
);
