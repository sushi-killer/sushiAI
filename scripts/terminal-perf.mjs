// Terminal-session benchmark: `npm run perf:terminal`.
// stdout carries one JSON object; progress and errors go to stderr.
// Flags: --quick, --only=output,input,renderer, --compare <before> <after>,
// --compare-base (see lib/perf-compare-base.mjs).
// Env: PERF_OUTPUT=<file> also writes the JSON; PERF_LINES, PERF_FRAME_BYTES,
// PERF_RTT_MS, PERF_KEYS, PERF_KEY_INTERVAL_MS override the config.
import { mkdtempSync, existsSync, writeFileSync, readFileSync } from "node:fs";
import { chmodSync, readdirSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import net from "node:net";
import path from "node:path";
import { spawn } from "node:child_process";
import { Duplex } from "node:stream";
import { createRequire } from "node:module";
import { monitorEventLoopDelay, performance } from "node:perf_hooks";
import { daemonBinary } from "./lib/daemon-binary.mjs";
import { compareBase } from "./lib/perf-compare-base.mjs";
import { createTerminalInput } from "../src/terminal-output.ts";

const require = createRequire(import.meta.url);
const root = process.cwd();
const log = (...args) => console.error("[terminal-perf]", ...args);

const SECTIONS = ["output", "renderer", "input"];
const PANEL = "terminal-perf";
// A daemon socket read is up to 64 KB: that many frames arrive in one turn.
const FRAMES_PER_TURN = 16;
const SNAPSHOT_LINES = 2000;

const round = (value, digits = 2) => Number(value.toFixed(digits));
const turn = () => new Promise((resolve) => setImmediate(resolve));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  if (!sorted.length) return 0;
  return sorted[
    Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)
  ];
}

// ---------------------------------------------------------------- compare

function numericLeaves(value, prefix = "", into = new Map()) {
  if (typeof value === "number" && Number.isFinite(value))
    into.set(prefix, value);
  else if (value && typeof value === "object")
    for (const [key, child] of Object.entries(value))
      numericLeaves(child, prefix ? `${prefix}.${key}` : key, into);
  return into;
}

function compare(beforeFile, afterFile) {
  const read = (file) => {
    const { config: _config, ...metrics } = JSON.parse(
      readFileSync(file, "utf8"),
    );
    return numericLeaves(metrics);
  };
  const before = read(beforeFile);
  const after = read(afterFile);
  const fmt = (value) => (value === undefined ? "-" : String(round(value, 2)));
  const rows = [
    "| metric | before | after | delta | delta % |",
    "| --- | ---: | ---: | ---: | ---: |",
  ];
  for (const key of new Set([...before.keys(), ...after.keys()])) {
    const a = before.get(key);
    const b = after.get(key);
    let delta = "-";
    let percent = "-";
    if (a !== undefined && b !== undefined) {
      const diff = b - a;
      delta = `${diff > 0 ? "+" : ""}${round(diff, 2)}`;
      percent =
        a === 0 ? "-" : `${diff > 0 ? "+" : ""}${round((diff / a) * 100, 1)}%`;
    }
    rows.push(`| ${key} | ${fmt(a)} | ${fmt(b)} | ${delta} | ${percent} |`);
  }
  console.log(rows.join("\n"));
}

// ----------------------------------------------------------------- config

function parseArgs(argv) {
  const options = { quick: false, only: [...SECTIONS], compare: undefined };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--quick") options.quick = true;
    else if (arg.startsWith("--only="))
      options.only = arg
        .slice("--only=".length)
        .split(",")
        .map((name) => name.trim())
        .filter(Boolean);
    else if (arg === "--compare")
      options.compare = [argv[index + 1], argv[index + 2]];
    else if (arg === "--compare-base") options.compareBase = true;
    else throw new Error(`unknown argument ${arg}`);
    if (arg === "--compare") index += 2;
  }
  for (const name of options.only)
    if (!SECTIONS.includes(name)) throw new Error(`unknown section ${name}`);
  return options;
}

function makeConfig(quick, env = process.env) {
  const number = (name, fallback) => {
    const value = Number(env[name]);
    return Number.isFinite(value) && value > 0 ? value : fallback;
  };
  return {
    lines: number("PERF_LINES", quick ? 20000 : 200000),
    frameBytes: number("PERF_FRAME_BYTES", 4096),
    rttMs: number("PERF_RTT_MS", 40),
    keys: number("PERF_KEYS", quick ? 20 : 60),
    keyIntervalMs: number("PERF_KEY_INTERVAL_MS", 30),
    quick,
  };
}

// ------------------------------------------------------------------ flood

const WORDS = ["build", "compile", "warning", "test", "pass", "fetch", "link"];

function floodLine(index) {
  const color = 31 + (index % 7);
  const wide = index % 3 === 0 ? " \u{1F600}" : "";
  const pad = "x".repeat(index % 40);
  const value = (index * 2654435761) % 100000;
  return `\x1b[${color}m[${index}]\x1b[0m ${WORDS[index % WORDS.length]} café ${pad}${wide} value=${value}`;
}

/** The flood as one buffer, the byte offset where each line starts, the
 * marker line that ends it and its split into daemon frames. */
function makeFlood({ lines, frameBytes }) {
  const marker = `PERF-DONE-${lines}`;
  const parts = [];
  const starts = new Float64Array(lines + 2);
  let offset = 0;
  for (let index = 0; index <= lines; index += 1) {
    const text = (index < lines ? floodLine(index) : marker) + "\r\n";
    starts[index] = offset;
    offset += Buffer.byteLength(text);
    parts.push(text);
  }
  starts[lines + 1] = offset;
  const buffer = Buffer.from(parts.join(""));
  const frames = [];
  for (let at = 0; at < buffer.length; at += frameBytes)
    frames.push(buffer.subarray(at, Math.min(buffer.length, at + frameBytes)));
  return { buffer, starts, frames, marker, lines: lines + 1 };
}

/** The last SNAPSHOT_LINES complete lines among the first `fed` bytes. */
function snapshotAt(flood, fed) {
  let low = 0;
  let high = flood.lines; // lines whose end <= fed
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (flood.starts[mid] <= fed) low = mid;
    else high = mid - 1;
  }
  const first = Math.max(0, low - SNAPSHOT_LINES);
  return flood.buffer
    .subarray(flood.starts[first], flood.starts[low])
    .toString("utf8");
}

/** A manager for createTerminalHandlers: attach answers with a snapshot of the
 * last lines fed so far, then the flood is pushed through the latest onBytes. */
function floodManager(flood, counters) {
  const state = { onBytes: undefined, fed: 0 };
  const manager = {
    request: async () => ({}),
    attach: async (_host, _id, _options, onBytes) => {
      state.onBytes = onBytes;
      counters.attaches += 1;
      onBytes(Buffer.from(snapshotAt(flood, state.fed)), {
        snapshot: true,
        cols: 80,
        rows: 24,
      });
      return { cols: 80, rows: 24, detach: async () => {} };
    },
  };
  async function feed() {
    for (let index = 0; index < flood.frames.length; index += 1) {
      const frame = flood.frames[index];
      state.fed += frame.length;
      state.onBytes?.(frame, {});
      if ((index + 1) % FRAMES_PER_TURN === 0) await turn();
    }
  }
  return { manager, feed };
}

function sendCounter(counters, onEvent) {
  return (channel, value) => {
    if (channel !== "daemon-terminal-data") return;
    counters.ipcMessages += 1;
    if (value.snapshot !== undefined) counters.snapshots += 1;
    const text = value.snapshot ?? value.data;
    if (text !== undefined) counters.sentBytes += Buffer.byteLength(text);
    onEvent(value);
  };
}

// ----------------------------------------------------------------- output

async function runOutput(config) {
  const {
    createTerminalHandlers,
  } = require("../electron/daemon/terminals.cjs");
  const flood = makeFlood(config);
  const counters = { ipcMessages: 0, snapshots: 0, sentBytes: 0, attaches: 0 };
  const { manager, feed } = floodManager(flood, counters);
  let handlers;
  const send = sendCounter(counters, (value) => {
    const text = value.snapshot ?? value.data;
    if (text !== undefined)
      setImmediate(() => handlers.ack(PANEL, Buffer.byteLength(text)));
  });
  handlers = createTerminalHandlers({ getManager: () => manager, send });
  await handlers.attach({
    panelId: PANEL,
    host: "local",
    sessionId: "perf",
    cols: 80,
    rows: 24,
  });
  const histogram = monitorEventLoopDelay({ resolution: 1 });
  histogram.enable();
  const started = performance.now();
  await feed();
  // Everything fed must drain: wait until the queue is empty and acked.
  for (let idle = 0; idle < 3;) {
    const before = counters.ipcMessages;
    await turn();
    idle = counters.ipcMessages === before ? idle + 1 : 0;
  }
  const wallMs = performance.now() - started;
  histogram.disable();
  await handlers.close();
  return {
    lines: config.lines,
    bytes: flood.buffer.length,
    ipcMessages: counters.ipcMessages,
    sentBytes: counters.sentBytes,
    wallMs: round(wallMs),
    eventLoopMaxMs: round(histogram.max / 1e6),
    eventLoopP99Ms: round(histogram.percentile(99) / 1e6),
    snapshots: counters.snapshots,
  };
}

// --------------------------------------------------------------- renderer

// TERMINAL_TEST_BROWSER, then Playwright's own build, then the newest
// installed headless shell or Chromium when Playwright expects another build.
function resolveBrowser(chromium) {
  if (process.env.TERMINAL_TEST_BROWSER)
    return process.env.TERMINAL_TEST_BROWSER;
  const expected = chromium.executablePath();
  if (expected && existsSync(expected)) return expected;
  const base =
    process.env.PLAYWRIGHT_BROWSERS_PATH ||
    path.join(homedir(), ".cache", "ms-playwright");
  const builds = [
    [
      "chromium_headless_shell-",
      "chrome-headless-shell-linux64/chrome-headless-shell",
    ],
    ["chromium-", "chrome-linux64/chrome"],
  ];
  let entries = [];
  try {
    entries = readdirSync(base);
  } catch {
    return undefined;
  }
  const found = [];
  for (const [prefix, binary] of builds) {
    for (const entry of entries) {
      const revision = Number(entry.slice(prefix.length));
      if (!entry.startsWith(prefix) || !Number.isInteger(revision)) continue;
      const file = path.join(base, entry, binary);
      if (existsSync(file)) found.push({ revision, file });
    }
  }
  // Newest revision first; the headless shell wins a tie (it is listed first).
  found.sort((a, b) => b.revision - a.revision);
  return found[0]?.file;
}

async function runRenderer(config) {
  const { chromium } = await import("playwright");
  const { createServer } = await import("vite");
  const {
    createTerminalHandlers,
  } = require("../electron/daemon/terminals.cjs");
  const flood = makeFlood(config);
  const counters = { ipcMessages: 0, snapshots: 0, sentBytes: 0, attaches: 0 };
  const server = await createServer({
    root,
    logLevel: "silent",
    server: { port: 5191, strictPort: false, hmr: false },
  });
  await server.listen();
  let browser;
  try {
    const executablePath = resolveBrowser(chromium);
    log("browser:", executablePath || "playwright default");
    browser = await chromium.launch({ executablePath });
    const page = await browser.newPage();
    page.setDefaultTimeout(30000);
    page.on("pageerror", (error) => log("page error:", error.message));
    const { manager, feed } = floodManager(flood, counters);
    let handlers;
    const send = sendCounter(counters, (value) => {
      void page
        .evaluate((event) => window.perfHarness.deliver(event), value)
        .catch(() => {});
    });
    handlers = createTerminalHandlers({ getManager: () => manager, send });
    await page.exposeFunction("perfAttach", (input) => handlers.attach(input));
    await page.exposeFunction("perfAck", (panelId, bytes) => {
      handlers.ack(panelId, bytes);
    });
    await page.route("**/terminal-perf.html", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: '<html><body><div id="host"></div></body></html>',
      }),
    );
    await page.goto(`${server.resolvedUrls.local[0]}terminal-perf.html`);
    await page.addScriptTag({
      type: "module",
      content: "import('/tests/terminal-perf-harness.tsx');",
    });
    await page.waitForSelector(".terminal-surface");
    // Attached once the first snapshot reached the page.
    for (let waited = 0; counters.snapshots < 1; waited += 50) {
      if (waited > 20000) throw new Error("the panel never attached");
      await sleep(50);
    }
    await sleep(300);
    await page.evaluate(() => window.perfHarness.begin());
    const started = performance.now();
    await feed();
    for (let waited = 0; ; waited += 25) {
      const done = await page.evaluate(
        (marker) => window.perfHarness.contains(marker),
        flood.marker,
      );
      if (done) break;
      if (waited > 600000) throw new Error("the flood never finished");
      await sleep(25);
    }
    const drainMs = performance.now() - started;
    const stats = await page.evaluate(() => window.perfHarness.end());
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("HeapProfiler.collectGarbage");
    const heap = await cdp.send("Runtime.getHeapUsage");
    return {
      drainMs: round(drainMs),
      frameP50Ms: round(stats.frameP50Ms),
      frameP95Ms: round(stats.frameP95Ms),
      frameMaxMs: round(stats.frameMaxMs),
      blockedMs: round(stats.blockedMs),
      longTaskMaxMs: round(stats.longTaskMaxMs),
      heapUsedMB: round(heap.usedSize / 1048576),
      domNodes: stats.domNodes,
      bufferLines: stats.bufferLines,
      ipcMessages: counters.ipcMessages,
      snapshots: counters.snapshots,
    };
  } finally {
    await browser?.close();
    await server.close();
  }
}

// ------------------------------------------------------------------ input

/** A socket that delivers every chunk in order, rttMs/2 later each way. */
function latencyProxy(socketPath, rttMs) {
  return () => {
    const half = rttMs / 2;
    const real = net.createConnection(socketPath);
    const duplex = new Duplex({
      read() {},
      write(chunk, _encoding, callback) {
        setTimeout(() => {
          if (!real.destroyed) real.write(chunk);
        }, half);
        callback();
      },
      final(callback) {
        setTimeout(() => real.end(), half);
        callback();
      },
      destroy(error, callback) {
        real.destroy();
        callback(error);
      },
    });
    real.on("connect", () => duplex.emit("connect"));
    real.on("data", (chunk) =>
      setTimeout(() => {
        if (!duplex.destroyed) duplex.push(chunk);
      }, half),
    );
    real.on("end", () =>
      setTimeout(() => {
        if (!duplex.destroyed) duplex.push(null);
      }, half),
    );
    real.on("error", (error) => duplex.destroy(error));
    return duplex;
  };
}

async function typeKeys({ client, home, config, label }) {
  const {
    createTerminalHandlers,
  } = require("../electron/daemon/terminals.cjs");
  const timeout = { timeoutMs: 60000 };
  const created = await client.request(
    "session.create",
    {
      cmd: ["/bin/cat"],
      cwd: home,
      cols: 80,
      rows: 24,
      idempotencyKey: `terminal-perf-${label}`,
    },
    timeout,
  );
  const manager = {
    request: (_host, method, params) => client.request(method, params, timeout),
    attach: (_host, id, _options, onBytes) => client.attach(id, onBytes),
  };
  const typedAt = [];
  const echoedAt = [];
  let handlers;
  handlers = createTerminalHandlers({
    getManager: () => manager,
    send: (_channel, value) => {
      const now = performance.now();
      const text = value.snapshot ?? value.data;
      if (text === undefined) return;
      if (value.data !== undefined)
        for (const character of value.data)
          if (character >= " " && character <= "~") echoedAt.push(now);
      handlers.ack(value.panelId, Buffer.byteLength(text));
    },
  });
  await handlers.attach({
    panelId: PANEL,
    host: "local",
    sessionId: created.id,
    cols: 80,
    rows: 24,
  });
  const errors = [];
  const input = createTerminalInput(
    (data) => handlers.write(PANEL, data),
    (message) => errors.push(message),
  );
  const start = performance.now() + 50;
  for (let key = 0; key < config.keys; key += 1) {
    const due = start + key * config.keyIntervalMs;
    await sleep(Math.max(0, due - performance.now()));
    typedAt.push(performance.now());
    input.send(String.fromCharCode(97 + (key % 26)));
  }
  const deadline = performance.now() + 30000;
  while (echoedAt.length < config.keys) {
    if (performance.now() > deadline)
      throw new Error(
        `${label}: ${echoedAt.length}/${config.keys} keys echoed${errors.length ? ` (${errors[0]})` : ""}`,
      );
    await sleep(10);
  }
  input.close();
  await handlers.detach(PANEL);
  await client.request(
    "session.close",
    { id: created.id, graceful: false },
    timeout,
  );
  const latencies = typedAt.map((typed, key) => echoedAt[key] - typed);
  return {
    keys: config.keys,
    p50Ms: round(percentile(latencies, 0.5)),
    p95Ms: round(percentile(latencies, 0.95)),
    maxMs: round(Math.max(...latencies)),
  };
}

async function runInput(config) {
  const { connectDaemon } = require("../electron/daemon/client.cjs");
  const {
    pidStart,
    stopPid,
    removeDir,
    until,
  } = require("../tests/helpers/deadline.cjs");
  const binary = daemonBinary(root);
  process.env.SUSHIAI_DAEMON_BIN = binary;
  const home = mkdtempSync(path.join(tmpdir(), "terminal-perf-"));
  chmodSync(home, 0o700);
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
  const holders = new Map();
  const clients = [];
  try {
    const socketPath = path.join(home, "daemon.sock");
    const open = (connect) =>
      until(async () => {
        if (!existsSync(socketPath)) return undefined;
        const client = await connectDaemon({
          socketPath,
          clientName: "terminal-perf",
          helloTimeoutMs: 60000,
          ...(connect ? { connect } : {}),
        }).catch(() => undefined);
        if (client) clients.push(client);
        return client;
      }, "the daemon socket");
    const local = await open();
    const remote = await open(latencyProxy(socketPath, config.rttMs));
    const result = {};
    for (const [label, client] of [
      ["local", local],
      ["remote", remote],
    ]) {
      const watch = async () => {
        const list = await client.request(
          "session.list",
          {},
          { timeoutMs: 60000 },
        );
        for (const entry of list)
          if (entry.holderPid && !holders.has(entry.holderPid))
            holders.set(entry.holderPid, pidStart(entry.holderPid));
      };
      const pending = typeKeys({ client, home, config, label });
      await sleep(500);
      await watch();
      result[label] = await pending;
    }
    return result;
  } finally {
    for (const client of clients) client.close();
    await stopPid(daemon.pid);
    for (const [holder, start] of holders) await stopPid(holder, { start });
    await exited;
    removeDir(home);
  }
}

// ------------------------------------------------------------------- main

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.compare) {
    if (!options.compare[0] || !options.compare[1])
      throw new Error("--compare needs <before.json> <after.json>");
    compare(...options.compare);
    return;
  }
  if (options.compareBase) {
    const args = process.argv
      .slice(2)
      .filter((arg) => arg !== "--compare-base");
    if (!(await compareBase({ root, args, table: compare })))
      process.exitCode = 1;
    return;
  }
  const config = makeConfig(options.quick);
  const result = { config };
  const runners = { output: runOutput, renderer: runRenderer, input: runInput };
  for (const name of SECTIONS) {
    if (!options.only.includes(name)) continue;
    log(`${name}...`);
    result[name] = await runners[name](config);
  }
  const json = JSON.stringify(result, null, 2);
  if (process.env.PERF_OUTPUT)
    writeFileSync(process.env.PERF_OUTPUT, `${json}\n`);
  console.log(json);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
