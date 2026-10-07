// End to end over a fake ssh that runs the "remote" side locally under a temp
// HOME: install a real sushiai from a hand-built manifest, connect through
// `sushiai proxy`, run a shell session and read its output.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { execFileSync } = require("node:child_process");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const { remoteConnectors } = require("../electron/daemon/connectors.cjs");
const { createHostInstaller } = require("../electron/host-setup.cjs");
const { setupHost } = require("../electron/host-setup.cjs");
const { makeHost } = require("./helpers/fake-host.cjs");
const { realDaemon } = require("./helpers/real-daemon.cjs");

const { binary, skip } = realDaemon();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check, ms = 20000, what = "condition") => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await sleep(25);
  }
  throw new Error(`timed out waiting for ${what}`);
};

test(
  "real daemon over a fake ssh: not installed, install, ready, shell session, echo",
  { skip, timeout: 120000 },
  async (t) => {
    // A short TMPDIR keeps the daemon's unix socket path under the OS limit.
    const previous = process.env.TMPDIR;
    process.env.TMPDIR = "/tmp";
    t.after(() => {
      if (previous === undefined) delete process.env.TMPDIR;
      else process.env.TMPDIR = previous;
    });
    const host = await makeHost(t);
    const profile = host.connections.list()[0];
    const bytes = fs.readFileSync(binary);
    const dist = fs.mkdtempSync(path.join("/tmp", "host-dist-"));
    t.after(() => fs.rmSync(dist, { recursive: true, force: true }));
    fs.mkdirSync(path.join(dist, "t"));
    fs.writeFileSync(path.join(dist, "t", "sushiai"), bytes, { mode: 0o755 });
    const platform = execFileSync("uname", ["-sm"], {
      encoding: "utf8",
    }).trim();
    const manifest = {
      [platform]: {
        target: "t",
        path: "t/sushiai",
        version: "0.0.0-test",
        size: bytes.length,
        sha256: createHash("sha256").update(bytes).digest("hex"),
      },
    };

    const manager = createDaemonManager({
      connectors: remoteConnectors(host.connections.profiles, {
        ssh: host.connections.ssh,
        args: (p) => host.connections.args(p),
      }),
      backoffMinMs: 50,
      backoffMaxMs: 200,
    });
    const hostName = profile.id;
    let sessionId;
    let handle;
    t.after(async () => {
      try {
        if (sessionId) {
          await handle?.detach();
          await manager.request(hostName, "session.close", {
            id: sessionId,
            graceful: false,
          });
        }
        await manager.request(hostName, "daemon.shutdown", {});
      } catch {
        // The daemon is already gone.
      }
      await sleep(400);
      manager.close();
    });
    const state = () => manager.states().find((s) => s.host === hostName);
    manager.start();
    // The profile has no autoConnect: the user connects it by hand.
    await manager.retry(hostName);
    await until(() => state().state === "failed", 20000, "not installed");
    assert.equal(state().reason, "not_installed");

    const installer = createHostInstaller({
      manager,
      connections: host.connections,
      manifest: () => ({ manifest, binDir: dist }),
      setup: (endpoint, options) =>
        setupHost(host.connections, endpoint, options),
    });
    const result = await installer.install(hostName);
    assert.equal(result.status, "installed");
    assert.equal(state().state, "ready");
    assert.ok(state().capabilities.includes("sessions"));

    const created = await manager.request(hostName, "session.create", {
      cwd: host.home,
      cols: 80,
      rows: 24,
      cmd: ["/bin/sh"],
      env: {},
      idempotencyKey: "e2e-1",
    });
    sessionId = created.id ?? created.sessionId;
    assert.ok(sessionId);
    let output = "";
    handle = await manager.attach(
      hostName,
      sessionId,
      { scrollback: 0 },
      (data) => {
        output += data.toString("utf8");
      },
    );
    await manager.request(hostName, "session.input", {
      id: sessionId,
      data: "echo MARKER_$((20+22))\n",
    });
    await until(() => output.includes("MARKER_42"), 15000, "echo output");
  },
);
