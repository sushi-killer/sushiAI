import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import net from "node:net";
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { performance } from "node:perf_hooks";
import { createRequire } from "node:module";
import assert from "node:assert/strict";
const require = createRequire(import.meta.url);
const { request } = require("../electron/herdr.cjs");

const root = path.resolve(process.argv[2] || process.cwd());
const output = path.resolve(
  process.argv[3] || "artifacts/herdr-app-benchmark.json",
);
const durationMs = Number(process.env.SUSHIAI_HERDR_APP_IDLE_MS || 120000);
const binary = process.env.SUSHIAI_HERDR_BINARY || "/usr/local/bin/herdr";
const transport = process.env.SUSHIAI_HERDR_APP_TRANSPORT || "local";
const profile = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-app-bench-"));
const backend = path.join(profile, "daemon.sock"),
  proxySocket = path.join(profile, "app.sock");
const config = path.join(profile, "config.toml");
await fs.writeFile(config, '[terminal]\nshell = "/bin/sh"\n');
const daemon = spawn(binary, ["server"], {
  env: {
    ...process.env,
    HOME: profile,
    XDG_CONFIG_HOME: `${profile}/config`,
    XDG_STATE_HOME: `${profile}/state`,
    HERDR_SOCKET_PATH: backend,
    HERDR_CLIENT_SOCKET_PATH: `${profile}/client.sock`,
    HERDR_CONFIG_PATH: config,
  },
  stdio: ["ignore", "ignore", "pipe"],
});
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check, label, timeout = 10000) {
  const end = performance.now() + timeout;
  while (performance.now() < end) {
    if (await check()) return;
    await delay(10);
  }
  throw new Error(`Timed out: ${label}`);
}
const clients = new Set(),
  rpc = [];
let proxyEnabled = true;
const proxy = net.createServer((client) => {
  if (!proxyEnabled) {
    client.destroy();
    return;
  }
  const upstream = net.createConnection(backend);
  clients.add(client);
  clients.add(upstream);
  let sent = "",
    received = "";
  client.on("data", (bytes) => {
    sent += bytes.toString("utf8");
    let newline;
    while ((newline = sent.indexOf("\n")) >= 0) {
      const line = sent.slice(0, newline);
      sent = sent.slice(newline + 1);
      const value = JSON.parse(line);
      rpc.push({
        id: value.id,
        method: value.method,
        requested: performance.now(),
        received: null,
      });
    }
  });
  upstream.on("data", (bytes) => {
    received += bytes.toString("utf8");
    let newline;
    while ((newline = received.indexOf("\n")) >= 0) {
      const line = received.slice(0, newline);
      received = received.slice(newline + 1);
      const value = JSON.parse(line),
        entry = rpc.findLast(
          (item) => item.id === value.id && item.received === null,
        );
      if (entry) entry.received = performance.now();
    }
  });
  for (const socket of [client, upstream]) {
    socket.on("error", () => {
      client.destroy();
      upstream.destroy();
    });
    socket.on("close", () => {
      clients.delete(socket);
      client.destroy();
      upstream.destroy();
    });
  }
  client.pipe(upstream);
  upstream.pipe(client);
});
let app;
let sourceCommit = process.env.SUSHIAI_HERDR_APP_SOURCE_COMMIT || null;
if (!sourceCommit) {
  try {
    sourceCommit = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    sourceCommit = null;
  }
}
const assets = {};
const report = {
  root,
  sourceCommit,
  assets,
  recordedAt: new Date().toISOString(),
  durationMs,
  transport,
  backgroundThrottling: false,
  pageErrors: [],
};
try {
  for (const file of await fs.readdir(path.join(root, "dist/assets"))) {
    if (/\.(js|css)$/.test(file))
      assets[file] = createHash("sha256")
        .update(await fs.readFile(path.join(root, "dist/assets", file)))
        .digest("hex");
  }
  await until(async () => {
    if (daemon.exitCode !== null) throw new Error("Isolated daemon exited");
    try {
      await request(backend, "ping", {}, 500);
      return true;
    } catch {
      return false;
    }
  }, "daemon");
  await new Promise((resolve) => proxy.listen(proxySocket, resolve));
  await fs.symlink(
    path.join(profile, "daemon-client.sock"),
    path.join(profile, "app-client.sock"),
  );
  let sshScript;
  if (transport === "ssh") {
    assert.ok(
      process.env.SUSHIAI_HERDR_BENCH_SSH_KEY &&
        process.env.SUSHIAI_HERDR_BENCH_SSH_PORT,
    );
    sshScript = path.join(profile, "ssh-benchmark");
    const key = process.env.SUSHIAI_HERDR_BENCH_SSH_KEY.replaceAll(
      "'",
      "'\\''",
    );
    await fs.writeFile(
      sshScript,
      `#!/bin/sh\nexec /usr/bin/ssh -F /dev/null -i '${key}' -o IdentitiesOnly=yes "$@"\n`,
      { mode: 0o700 },
    );
  }
  app = await electron.launch({
    ...(process.env.SUSHIAI_EXECUTABLE
      ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
      : {}),
    args: ["--no-sandbox", "."],
    cwd: root,
    env: {
      ...process.env,
      SUSHIAI_TEST_WINDOW: "hidden",
      BRIDGE_DATA_DIR: `${profile}/app`,
      BRIDGE_DEV_URL: "",
      HERDR_SOCKET_PATH:
        transport === "ssh" ? `${profile}/no-local.sock` : proxySocket,
      ...(sshScript ? { SUSHIAI_TEST_SSH: sshScript } : {}),
    },
  });
  app.process().stderr.on("data", (bytes) => {
    report.stderr = ((report.stderr || "") + bytes).slice(-4000);
  });
  const page = await app.firstWindow();
  await app.evaluate(({ BrowserWindow }) => {
    BrowserWindow.getAllWindows()[0].webContents.setBackgroundThrottling(false);
  });
  page.on("pageerror", (error) => report.pageErrors.push(error.message));
  await page.waitForSelector(".sidebar");
  let endpoint = proxySocket;
  if (transport === "ssh") {
    endpoint = await page.evaluate(
      async ({ host, port, socket }) => {
        const profile = await window.bridge.connectionsSave({
          name: "SSH benchmark",
          host,
          port,
          socket,
        });
        const endpoint = `ssh:${profile.id}`;
        await window.bridge.connectionsConnect(endpoint);
        return endpoint;
      },
      {
        host: `${os.userInfo().username}@${[127, 0, 0, 1].join(".")}`,
        port: Number(process.env.SUSHIAI_HERDR_BENCH_SSH_PORT),
        socket: proxySocket,
      },
    );
    await page.reload();
    await page.waitForSelector(".sidebar");
  }
  await until(
    () =>
      rpc.some(
        (item) => item.method === "session.snapshot" && item.received !== null,
      ),
    "initial snapshot",
  );
  await delay(3000);
  const idleStart = performance.now();
  await delay(durationMs);
  const idleEnd = performance.now();
  report.idle = {
    elapsedMs: idleEnd - idleStart,
    snapshotCount: rpc.filter(
      (item) =>
        item.method === "session.snapshot" &&
        item.requested >= idleStart &&
        item.requested < idleEnd,
    ).length,
  };
  const detections = [];
  for (let i = 0; i < 8; i++) {
    await delay(113 + i * 197);
    const label = `External benchmark ${i + 1}`;
    await page.evaluate((label) => {
      const armedAt = performance.now();
      window.__herdrBenchmarkPaint = null;
      const observer = new MutationObserver(() => {
        const row = [...document.querySelectorAll(".workspace-name")].find(
          (element) => element.textContent.includes(label),
        );
        if (!row) return;
        observer.disconnect();
        const mutationMs = performance.now() - armedAt;
        requestAnimationFrame(() => {
          const bounds = row.getBoundingClientRect();
          window.__herdrBenchmarkPaint = {
            observedMs: performance.now() - armedAt,
            mutationMs,
            width: bounds.width,
            height: bounds.height,
          };
        });
      });
      observer.observe(document.querySelector(".sidebar"), {
        childList: true,
        subtree: true,
        characterData: true,
      });
    }, label);
    const start = performance.now();
    const created = await request(backend, "workspace.create", {
      label,
      cwd: profile,
      focus: false,
    });
    const responseAt = performance.now();
    await page
      .locator(".workspace-name")
      .filter({ hasText: label })
      .waitFor({ timeout: 10000 });
    const displayedAt = performance.now();
    await page.waitForFunction(() => !!window.__herdrBenchmarkPaint);
    const painted = await page.evaluate(() => window.__herdrBenchmarkPaint);
    const geometry = await page
      .locator(".workspace-name")
      .filter({ hasText: label })
      .evaluate((element) => {
        const range = document.createRange();
        range.selectNodeContents(element);
        const bounds = range.getBoundingClientRect();
        return { width: bounds.width, height: bounds.height };
      });
    detections.push({
      requestToRenderedMs: displayedAt - start,
      acknowledgedToRenderedMs: displayedAt - responseAt,
      label,
      geometry,
      requestToAnimationFrameMs: painted.observedMs,
      requestToDomMs: painted.mutationMs,
    });
    await request(backend, "workspace.close", {
      workspace_id: created.workspace.workspace_id,
    });
  }
  const sorted = detections
    .map((item) => item.requestToRenderedMs)
    .sort((a, b) => a - b);
  report.externalChange = {
    p50Ms: sorted[Math.floor(sorted.length / 2)],
    p95Ms: sorted.at(-1),
    samples: detections,
    animationFrameP50Ms: [
      ...detections.map((item) => item.requestToAnimationFrameMs),
    ].sort((a, b) => a - b)[Math.floor(detections.length / 2)],
    animationFrameP95Ms: Math.max(
      ...detections.map((item) => item.requestToAnimationFrameMs),
    ),
    domP50Ms: [...detections.map((item) => item.requestToDomMs)].sort(
      (a, b) => a - b,
    )[Math.floor(detections.length / 2)],
    domP95Ms: Math.max(...detections.map((item) => item.requestToDomMs)),
  };
  report.structuredIpc = await page.evaluate(async (endpoint) => {
    try {
      await window.bridge.herdr(endpoint, "pane.split", {
        target_pane_id: "w999999:p999999",
        direction: "right",
      });
      return { rejected: false };
    } catch (error) {
      return { rejected: true, code: error.code, message: error.message };
    }
  }, endpoint);
  const reconnectStart = performance.now();
  proxyEnabled = false;
  for (const socket of clients) socket.destroy();
  const reconnectWorkspace = await request(backend, "workspace.create", {
    label: "Reconnect benchmark",
    cwd: profile,
    focus: false,
  });
  await delay(500);
  proxyEnabled = true;
  await page
    .locator(".workspace-name")
    .filter({ hasText: "Reconnect benchmark" })
    .waitFor({ timeout: 15000 });
  report.reconnect = {
    listRestored: true,
    disruptionToRenderedMs: performance.now() - reconnectStart,
    fullSnapshots: rpc.filter(
      (item) =>
        item.method === "session.snapshot" && item.requested >= reconnectStart,
    ).length,
  };
  await request(backend, "workspace.close", {
    workspace_id: reconnectWorkspace.workspace.workspace_id,
  });
  const persistent = await request(backend, "workspace.create", {
    label: "Persistence benchmark",
    cwd: profile,
    focus: false,
  });
  const persistentPane = persistent.root_pane.pane_id;
  const pidFile = path.join(profile, "shell.pid");
  await request(backend, "pane.send_input", {
    pane_id: persistentPane,
    text: `printf '%s' "$$" > '${pidFile}'`,
    keys: ["Enter"],
  });
  await until(async () => {
    try {
      return /^\d+$/.test(await fs.readFile(pidFile, "utf8"));
    } catch {
      return false;
    }
  }, "shell PID");
  const shellPid = await fs.readFile(pidFile, "utf8");
  await page
    .locator(".workspace-name")
    .filter({ hasText: "Persistence benchmark" })
    .waitFor({ timeout: 10000 });
  await page
    .locator(".workspace-name")
    .filter({ hasText: "Persistence benchmark" })
    .click();
  await delay(300);
  report.window = await app.evaluate(({ BrowserWindow }) => {
    const win = BrowserWindow.getAllWindows()[0];
    return {
      visible: win.isVisible(),
      focused: win.isFocused(),
      contentSize: win.getContentSize(),
    };
  });
  await fs.mkdir(path.dirname(output), { recursive: true });
  const screenshot = output.replace(/\.json$/, ".png");
  await page.screenshot({ path: screenshot });
  report.screenshot = screenshot;
  await app.close();
  app = null;
  const afterCloseFile = path.join(profile, "after-close.pid");
  await request(backend, "pane.send_input", {
    pane_id: persistentPane,
    text: `printf '%s' "$$" > '${afterCloseFile}'`,
    keys: ["Enter"],
  });
  await until(async () => {
    try {
      return /^\d+$/.test(await fs.readFile(afterCloseFile, "utf8"));
    } catch {
      return false;
    }
  }, "shell after app close");
  const afterPid = await fs.readFile(afterCloseFile, "utf8");
  assert.equal(afterPid, shellPid);
  report.appClosePersistence = {
    sameShellPid: true,
    shellPid,
    beforePaneId: persistentPane,
    afterPaneId: (
      await request(backend, "session.snapshot")
    ).snapshot.panes.find((pane) => pane.pane_id === persistentPane)?.pane_id,
  };
  await request(backend, "workspace.close", {
    workspace_id: persistent.workspace.workspace_id,
  });
} catch (error) {
  report.error = error.message;
  process.exitCode = 1;
} finally {
  if (app) await app.close();
  for (const socket of clients) socket.destroy();
  await new Promise((resolve) => proxy.close(resolve));
  if (daemon.exitCode === null) {
    await request(backend, "server.stop", {}, 2000).catch(() => daemon.kill());
    await until(() => daemon.exitCode !== null, "daemon stop", 5000).catch(() =>
      daemon.kill(),
    );
  }
  report.rpc = rpc;
  await fs.mkdir(path.dirname(output), { recursive: true });
  await fs.writeFile(output, JSON.stringify(report, null, 2) + "\n");
  await fs.rm(profile, { recursive: true, force: true });
  console.log(
    JSON.stringify(
      {
        output,
        idle: report.idle,
        externalChange: report.externalChange,
        error: report.error,
      },
      null,
      2,
    ),
  );
}
