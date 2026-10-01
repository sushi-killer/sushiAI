const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { performance } = require("node:perf_hooks");
const { request } = require("../electron/herdr.cjs");
const { Connections } = require("../electron/connections.cjs");
const {
  assertHerdrCompatibility,
} = require("../electron/herdr-compatibility.cjs");
const { HerdrEvents } = require("../electron/herdr-events.cjs");
const { installPinnedHerdr } = require("../electron/herdr-install.cjs");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(check, label, timeout = 10000) {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    if (await check()) return;
    await sleep(10);
  }
  throw new Error(`Timed out: ${label}`);
}

function percentiles(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  return {
    p50: sorted[Math.floor(sorted.length * 0.5)],
    p95: sorted[Math.floor(sorted.length * 0.95)],
    samples: sorted.length,
  };
}

(async () => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "herdr-runtime-live-"),
  );
  const socket = path.join(directory, "api.sock");
  const connections = new Connections(path.join(directory, "app"));
  let daemon, events;
  try {
    const binary = process.env.SUSHIAI_HERDR_BINARY || "/usr/local/bin/herdr";
    const env = {
      ...process.env,
      HOME: directory,
      XDG_CONFIG_HOME: path.join(directory, "config"),
      XDG_STATE_HOME: path.join(directory, "state"),
      HERDR_SOCKET_PATH: socket,
      HERDR_CONFIG_PATH: path.join(directory, "config.toml"),
    };
    await fs.writeFile(
      env.HERDR_CONFIG_PATH,
      '[terminal]\nshell = "/bin/sh"\n',
    );
    daemon = spawn(binary, ["server"], {
      env,
      stdio: ["ignore", "ignore", "pipe"],
    });
    let diagnostic = "";
    daemon.stderr.on("data", (data) => {
      diagnostic = (diagnostic + data).slice(-4000);
    });
    await waitFor(async () => {
      if (daemon.exitCode !== null)
        throw new Error(`Isolated daemon exited: ${diagnostic}`);
      try {
        await request(socket, "ping", {}, 500);
        return true;
      } catch {
        return false;
      }
    }, "isolated daemon");
    const status = await assertHerdrCompatibility({
      endpoint: socket,
      connections,
      binary,
    });
    const notifications = [];
    events = new HerdrEvents({
      getConnections: () => connections,
      send: (_, event) =>
        notifications.push({ ...event, received: performance.now() }),
    });
    events.subscribe(socket, "live-runtime-check");
    await waitFor(
      () => notifications.some((event) => event.type === "connected"),
      "native subscription",
    );
    const createdAt = performance.now();
    const created = await request(socket, "workspace.create", {
      label: "Runtime verification",
      cwd: directory,
      focus: false,
      env: { SUSHIAI_RUNTIME_PROBE: "value 🍣" },
    });
    const workspaceId = created.workspace.workspace_id;
    const paneId = created.root_pane.pane_id;
    await waitFor(
      () =>
        notifications.some(
          (event) =>
            event.type === "changed" && event.event === "workspace_created",
        ),
      "external workspace event",
    );
    const detectionMs =
      notifications.find((event) => event.event === "workspace_created")
        .received - createdAt;
    await sleep(300);
    await request(socket, "pane.send_input", {
      pane_id: paneId,
      text: "printf 'ENV:%s\\n' \"$SUSHIAI_RUNTIME_PROBE\"",
    });
    await request(socket, "pane.send_input", {
      pane_id: paneId,
      keys: ["Enter"],
    });
    await waitFor(
      async () =>
        (
          await request(socket, "pane.read", {
            pane_id: paneId,
            source: "recent",
            format: "text",
            strip_ansi: true,
          })
        ).read.text.includes("ENV:value 🍣"),
      "launch env and Unicode",
    );
    const beforeRelease = await request(socket, "session.snapshot");
    events.unsubscribe(socket, "live-runtime-check");
    events.close();
    events = null;
    await sleep(100);
    const afterRelease = await request(socket, "session.snapshot");
    assert.ok(
      beforeRelease.snapshot.panes.some((pane) => pane.pane_id === paneId),
    );
    assert.ok(
      afterRelease.snapshot.panes.some((pane) => pane.pane_id === paneId),
    );
    const samples = [];
    for (let i = 0; i < 100; i++) {
      const start = performance.now();
      await request(socket, "session.snapshot");
      samples.push(performance.now() - start);
    }
    const concurrent = [];
    await Promise.all(
      Array.from({ length: 8 }, async () => {
        for (let i = 0; i < 20; i++) {
          const start = performance.now();
          await request(socket, "session.snapshot");
          concurrent.push(performance.now() - start);
        }
      }),
    );
    let installed;
    if (process.env.SUSHIAI_VERIFY_HERDR_INSTALL === "1") {
      installed = await installPinnedHerdr(path.join(directory, "managed"));
      await assertHerdrCompatibility({
        endpoint: socket,
        connections,
        binary: installed.binary,
      });
    }
    await request(socket, "workspace.close", { workspace_id: workspaceId });
    console.log(
      JSON.stringify(
        {
          passed: true,
          daemon: status.daemon,
          cli: status.cli,
          nativeEventDetectionMs: detectionMs,
          snapshotSequentialMs: percentiles(samples),
          snapshot8ClientsMs: percentiles(concurrent),
          installed: installed || false,
          checks: [
            "isolated daemon and CLI compatibility",
            "native events",
            "launch env and Unicode",
            "unsubscribe preserves process",
            "request latency",
          ],
        },
        null,
        2,
      ),
    );
  } finally {
    events?.close();
    await connections.close();
    if (daemon && daemon.exitCode === null) {
      await request(socket, "server.stop", {}, 2000).catch(() => daemon.kill());
      await waitFor(
        () => daemon.exitCode !== null,
        "isolated daemon shutdown",
        5000,
      ).catch(() => daemon.kill());
    }
    await fs.rm(directory, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
