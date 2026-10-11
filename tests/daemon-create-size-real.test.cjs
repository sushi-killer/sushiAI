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

const { realDaemon } = require("./helpers/real-daemon.cjs");
const {
  DEADLINE_MS,
  until,
  pidStart,
  stopPid,
  removeDir,
} = require("./helpers/deadline.cjs");
const { binary, skip } = realDaemon();

test(
  "a launch from a measured 40x9 pane starts the session at 40x9 and never resizes it",
  { skip },
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
    const timeout = { timeoutMs: DEADLINE_MS };
    const holders = new Map();
    let client;
    try {
      const socketPath = path.join(home, "daemon.sock");
      client = await until(async () => {
        if (!fs.existsSync(socketPath)) return undefined;
        return connectDaemon({
          socketPath,
          clientName: "size-test",
          helloTimeoutMs: DEADLINE_MS,
        }).catch(() => undefined);
      }, "the daemon socket");
      const methods = [];
      const manager = {
        request: (_host, method, params) => {
          methods.push(method);
          return client.request(method, params, timeout);
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
      for (const entry of await client.request("session.list", {}, timeout))
        if (entry.holderPid)
          holders.set(entry.holderPid, pidStart(entry.holderPid));

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
      const text = () => sent.map((m) => m.snapshot ?? m.data ?? "").join("");
      await handlers.write("p1", "stty size\r");
      // Wait for the output line itself: the echo of the typed command can
      // arrive first and holds no size.
      await until(() => /^\d+ \d+\r?$/m.test(text()), "stty output");
      assert.match(text(), /^9 40\r?$/m);
      assert.ok(!methods.includes("session.resize"), "no resize was sent");
      await client.request(
        "session.close",
        { id: sessionId, graceful: false },
        timeout,
      );
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
