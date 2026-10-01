const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { Connections, quote } = require("../electron/connections.cjs");
const { request } = require("../electron/herdr.cjs");
const { openHerdrStream } = require("../electron/terminal-stream.cjs");
const { OUTPUT_CREDIT_BYTES } = require("../electron/terminal-flow.cjs");
const {
  startStreamDaemon,
  streamTestBinary,
} = require("./terminal-stream-fixture.cjs");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const seconds = Number(process.env.SUSHIAI_STREAM_LOAD_SECONDS || 600);
if (!Number.isFinite(seconds) || seconds < 1)
  throw new Error("Invalid load duration.");

function loadFixture() {
  return [
    "import os,time",
    "iteration=0",
    `deadline=time.monotonic()+${seconds + 5}`,
    "while time.monotonic()<deadline:",
    " lines=['\\x1b['+str(31+(iteration+j)%7)+'mUnicode Привет 🌍 '+str(iteration+j)+' '+''.join(chr(33+(iteration+j+k)%90) for k in range(90))+'\\x1b[0m\\r\\n' for j in range(250)]",
    " os.write(1,''.join(lines).encode())",
    " iteration+=250",
    " time.sleep(0.01)",
  ].join("\n");
}

async function nodeLoad() {
  const daemon = await startStreamDaemon(streamTestBinary());
  const connections = new Connections(daemon.directory);
  await connections.init();
  const streams = [];
  const workspaceIds = [];
  const observations = [];
  let selected = false;
  let bytes = 0;
  let failure;
  const held = [];
  const acknowledge = (stream, event) => {
    if (event.sequence) stream.ack(event.streamId, event.sequence);
  };
  try {
    for (let index = 0; index < 2; index++) {
      const created = await request(daemon.socket, "workspace.create", {
        label: `isolated stream load ${index + 1}`,
        cwd: daemon.directory,
        focus: false,
        env: { LANG: "en_US.UTF-8", LC_ALL: "en_US.UTF-8" },
      });
      workspaceIds.push(created.workspace.workspace_id);
      let stream;
      stream = await openHerdrStream({
        endpoint: daemon.socket,
        panelId: `load-${index}`,
        target: created.root_pane.pane_id,
        cols: 110,
        rows: 35,
        binary: daemon.binary,
        connections,
        send: (_channel, event) => {
          if (event.error) failure = new Error(event.error);
          bytes += Buffer.byteLength(event.data);
          if (event.sequence) {
            if (selected) held.push({ stream, event });
            else setTimeout(() => acknowledge(stream, event), 5);
          }
        },
      });
      streams.push(stream);
      await stream.proc.write(`python3 -c ${quote(loadFixture())}\r`);
    }
    const started = Date.now();
    for (let tick = 0; Date.now() - started < seconds * 1000; tick++) {
      if (failure) throw failure;
      selected = tick % 30 < 15;
      if (!selected)
        while (held.length) {
          const packet = held.shift();
          acknowledge(packet.stream, packet.event);
        }
      await sleep(1000);
      const flows = streams.map((stream) => stream.flowStats());
      for (const flow of flows) {
        assert.ok(flow.output.bytes <= OUTPUT_CREDIT_BYTES);
        assert.ok(flow.parserBytes <= flow.limits.parserBytes);
      }
      const cliRssBytes = flows.map(
        (flow) =>
          Number(
            execFileSync("ps", ["-o", "rss=", "-p", String(flow.pid)], {
              encoding: "utf8",
            }).trim(),
          ) * 1024,
      );
      observations.push({
        elapsedSeconds: (Date.now() - started) / 1000,
        rssBytes: process.memoryUsage().rss,
        cliRssBytes,
        outputBytes: flows.map((flow) => flow.output.bytes),
        parserBytes: flows.map((flow) => flow.parserBytes),
        heldBytes: held.reduce(
          (total, packet) => total + Buffer.byteLength(packet.event.data),
          0,
        ),
      });
      assert.ok(
        observations.at(-1).heldBytes <= streams.length * OUTPUT_CREDIT_BYTES,
      );
    }
    selected = false;
    while (held.length) {
      const packet = held.shift();
      acknowledge(packet.stream, packet.event);
    }
    await sleep(250);
    const samples = observations.slice(10);
    const first = samples.slice(0, Math.max(1, Math.floor(samples.length / 4)));
    const last = samples.slice(-Math.max(1, Math.floor(samples.length / 4)));
    const average = (items, value) =>
      items.reduce((sum, item) => sum + value(item), 0) / items.length;
    const rssGrowthBytes =
      average(last, (item) => item.rssBytes) -
      average(first, (item) => item.rssBytes);
    const report = {
      passed: true,
      durationSeconds: seconds,
      panels: streams.length,
      consumer:
        "5 ms acknowledgements, 15 seconds selection hold every 30 seconds; no xterm renderer",
      deliveredBytes: bytes,
      maxOutputCreditBytes: Math.max(
        ...observations.flatMap((item) => item.outputBytes),
      ),
      maxHeldBytes: Math.max(...observations.map((item) => item.heldBytes)),
      maxParserBytes: Math.max(
        ...observations.flatMap((item) => item.parserBytes),
      ),
      rssGrowthBytes,
      first: observations[0],
      last: observations.at(-1),
      cliRssPeakBytes: streams.map((_stream, index) =>
        Math.max(...observations.map((item) => item.cliRssBytes[index])),
      ),
    };
    await fs.mkdir(path.join(process.cwd(), "artifacts"), { recursive: true });
    await fs.writeFile(
      path.join(process.cwd(), "artifacts/terminal-flow-load.json"),
      JSON.stringify(report, null, 2) + "\n",
    );
    console.log(JSON.stringify(report, null, 2));
  } finally {
    for (const stream of streams) stream.proc.kill();
    for (const workspaceId of workspaceIds)
      await request(daemon.socket, "workspace.close", {
        workspace_id: workspaceId,
      }).catch(() => {});
    await connections.close();
    await daemon.close();
  }
}

async function electronLoad() {
  const { _electron: electron } = require("playwright");
  const ts = require("typescript");
  const root = path.resolve(__dirname, "..");
  const fixture = await fs.mkdtemp(
    path.join(os.tmpdir(), "sushiai-xterm-load-"),
  );
  const daemon = await startStreamDaemon(streamTestBinary());
  const panes = [];
  const samples = [];
  const samplesFile = path.join(
    root,
    "artifacts/terminal-flow-electron-samples.json",
  );
  await fs.mkdir(path.dirname(samplesFile), { recursive: true });
  let app;
  try {
    for (const name of [
      "terminal-output",
      "terminal-copy",
      "terminal-interactions",
    ]) {
      const source = await fs.readFile(
        path.join(root, "src", `${name}.ts`),
        "utf8",
      );
      await fs.writeFile(
        path.join(fixture, `${name}.js`),
        ts.transpileModule(
          source.replaceAll('"./terminal-copy"', '"./terminal-copy.js"'),
          {
            compilerOptions: {
              target: ts.ScriptTarget.ES2022,
              module: ts.ModuleKind.ESNext,
            },
          },
        ).outputText,
      );
    }
    await fs.writeFile(
      path.join(fixture, "preload.cjs"),
      `
      const { contextBridge, ipcRenderer } = require('electron');
      contextBridge.exposeInMainWorld('bridge', {
        terminalOpen: options => ipcRenderer.invoke('terminal-open', options),
        terminalWrite: (id, data) => ipcRenderer.invoke('terminal-write', id, data),
        terminalAck: (id, token, sequence) => ipcRenderer.invoke('terminal-ack', id, token, sequence),
        terminalScroll: (id, direction, lines, position) => ipcRenderer.invoke('terminal-scroll', id, direction, lines, position),
        loadStats: () => ipcRenderer.invoke('load-stats'),
        loadCollect: () => ipcRenderer.invoke('load-collect'),
        onTerminal: callback => ipcRenderer.on('terminal-data', (_event, value) => callback(value)),
      });
    `,
    );
    await fs.writeFile(
      path.join(fixture, "main.cjs"),
      `
      const { app, BrowserWindow, ipcMain } = require('electron');
      const v8 = require('node:v8');
      const { Connections } = require(${JSON.stringify(path.join(root, "electron/connections.cjs"))});
      const { registerTerminalIpc } = require(${JSON.stringify(path.join(root, "electron/ipc/terminals.cjs"))});
      app.setPath('userData', ${JSON.stringify(path.join(fixture, "profile"))});
      const terminals = new Map();
      let window, ipc;
      const connections = new Connections(${JSON.stringify(fixture)});
      globalThis.streamLoad = { terminals };
      app.whenReady().then(async () => {
        await connections.init();
        window = new BrowserWindow({ show: false, width: 1000, height: 950,
          webPreferences: { preload: ${JSON.stringify(path.join(fixture, "preload.cjs"))},
            sandbox: true, contextIsolation: true, nodeIntegration: false } });
        ipc = registerTerminalIpc({
          handle: (name, handler) => ipcMain.handle(name, (_event, ...args) => handler(...args)),
          send: (name, value) => { if (!window.isDestroyed()) window.webContents.send(name, value); },
          getConnections: () => connections,
          executable: () => ${JSON.stringify(daemon.binary)},
          id: value => value, terminals, terminalPending: new Map(),
        });
        ipcMain.handle('load-stats', () => ({ pid: process.pid, memory: process.memoryUsage(),
          heap: v8.getHeapStatistics(), spaces: v8.getHeapSpaceStatistics(),
          rendererPid: window.webContents.getOSProcessId(),
          flows: [...terminals.values()].map(entry => entry.flowStats()), visible: window.isVisible() }));
        ipcMain.handle('load-collect', () => { const before = process.memoryUsage();
          if (typeof globalThis.gc !== 'function') throw new Error('Diagnostic GC is unavailable');
          globalThis.gc(); return { before, after: process.memoryUsage() }; });
        await window.loadFile(${JSON.stringify(path.join(fixture, "index.html"))});
      });
      app.on('before-quit', () => {
        ipc?.close();
        for (const entry of terminals.values()) entry.proc.kill();
      });
      app.on('window-all-closed', () => app.quit());
    `,
    );
    await fs.writeFile(
      path.join(fixture, "index.html"),
      `
      <html><head><link rel="stylesheet" href="${pathToFileURL(path.join(root, "node_modules/@xterm/xterm/css/xterm.css"))}">
      <style>body { margin: 0; background: #161a20; color: white; }
      .terminal { height: 440px; margin: 12px; }</style></head>
      <body><div id="first" class="terminal"></div><div id="second" class="terminal"></div>
      <script src="${pathToFileURL(path.join(root, "node_modules/@xterm/xterm/lib/xterm.js"))}"></script>
      <script type="module">
        import { createTerminalOutput, createTerminalInput } from './terminal-output.js';
        import { installTerminalInteractions } from './terminal-interactions.js';
        window.loadTerminals = [];
        window.loadErrors = [];
        window.loadBytes = 0;
        window.createLoadTerminal = async (options, index) => {
          const terminal = new Terminal({ cols: 110, rows: 26, scrollback: 0, fontSize: 12 });
          const element = document.getElementById(index ? 'second' : 'first');
          terminal.open(element);
          const output = createTerminalOutput({ streamId: options.streamId,
            write: (data, done) => { window.loadBytes += new TextEncoder().encode(data).length; terminal.write(data, done); },
            reset: () => terminal.reset(),
            ack: (token, sequence) => window.bridge.terminalAck(options.panelId, token, sequence).catch(error => window.loadErrors.push(error.message)),
            fail: message => window.loadErrors.push(message),
          });
          window.bridge.onTerminal(event => {
            if (event.panelId !== options.panelId || !output.accepts(event.streamId)) return;
            if (event.error) window.loadErrors.push(event.error);
            if (event.sequence) output.push(event);
          });
          const input = createTerminalInput(data => window.bridge.terminalWrite(options.panelId, data),
            message => window.loadErrors.push(message));
          terminal.onData(data => input.send(data));
          const interactions = installTerminalInteractions(terminal, element, data => input.send(data),
            (direction, lines, position) => window.bridge.terminalScroll(options.panelId, direction, lines, position).catch(error => window.loadErrors.push(error.message)),
            active => output.pause(active));
          window.loadTerminals.push({ terminal, output, input, interactions, element });
          await window.bridge.terminalOpen(options);
          output.start();
        };
      </script></body></html>
    `,
    );
    app = await electron.launch({
      timeout: 20000,
      args: [path.join(fixture, "main.cjs")],
      ...(process.env.SUSHIAI_STREAM_LOAD_GC_DIAGNOSTIC
        ? { args: ["--js-flags=--expose-gc", path.join(fixture, "main.cjs")] }
        : {}),
      ...(process.env.SUSHIAI_EXECUTABLE
        ? { executablePath: process.env.SUSHIAI_EXECUTABLE }
        : {}),
      env: {
        ...process.env,
        SUSHIAI_TEST_WINDOW: "hidden",
        HERDR_SOCKET_PATH: daemon.socket,
      },
    });
    const page = await app.firstWindow({ timeout: 20000 });
    const rendererCdp = await page.context().newCDPSession(page);
    console.error("Electron load: window ready");
    page.on("pageerror", (error) => {
      throw error;
    });
    await page.waitForFunction(
      () => typeof window.createLoadTerminal === "function",
    );
    for (let index = 0; index < 2; index++) {
      const created = await request(daemon.socket, "workspace.create", {
        label: `isolated Electron stream load ${index + 1}`,
        cwd: daemon.directory,
        focus: false,
        env: { LANG: "en_US.UTF-8", LC_ALL: "en_US.UTF-8" },
      });
      panes.push({
        workspace: created.workspace.workspace_id,
        pane: created.root_pane.pane_id,
      });
      await page.evaluate(
        async ({ options, index }) => window.createLoadTerminal(options, index),
        {
          index,
          options: {
            endpoint: daemon.socket,
            panelId: `load-${index}`,
            herdrId: panes[index].pane,
            streamId: `load-${index}`,
            cols: 110,
            rows: 26,
          },
        },
      );
    }
    await sleep(500);
    console.error("Electron load: two panes attached");
    await page.evaluate(
      async (text) => {
        for (const entry of window.loadTerminals) entry.terminal.paste(text);
      },
      `python3 -c ${quote(loadFixture())}\r`,
    );
    const started = Date.now();
    for (let tick = 0; Date.now() - started < seconds * 1000; tick++) {
      const selected = tick % 30 < 15;
      if (tick % 15 === 0)
        await page.evaluate(async (selected) => {
          for (const entry of window.loadTerminals) {
            if (!selected) {
              entry.interactions.resume();
              continue;
            }
            const rect = entry.element
              .querySelector(".xterm-screen")
              .getBoundingClientRect();
            entry.element.dispatchEvent(
              new MouseEvent("mousedown", {
                button: 0,
                buttons: 1,
                clientX: rect.left + 2,
                clientY: rect.top + 2,
                bubbles: true,
              }),
            );
            // Complete the write already in xterm before choosing the frozen text.
            await new Promise((resolve) => entry.terminal.write("", resolve));
            entry.terminal.select(0, entry.terminal.buffer.active.viewportY, 7);
            document.dispatchEvent(
              new MouseEvent("mouseup", { button: 0, bubbles: true }),
            );
          }
        }, selected);
      await sleep(1000);
      const main = await page.evaluate(() => window.bridge.loadStats());
      const renderer = await page.evaluate(() => ({
        queues: window.loadTerminals.map((entry) => entry.output.stats),
        errors: window.loadErrors,
        deliveredBytes: window.loadBytes,
        selections: window.loadTerminals.map((entry) =>
          entry.terminal.hasSelection(),
        ),
        replacementCharacters: window.loadTerminals.some((entry) =>
          Array.from(
            { length: entry.terminal.buffer.active.length },
            (_, index) =>
              entry.terminal.buffer.active.getLine(index).translateToString(),
          )
            .join("")
            .includes("\ufffd"),
        ),
      }));
      const rendererMemory = await rendererCdp.send("Runtime.getHeapUsage");
      assert.equal(main.visible, false);
      assert.deepEqual(renderer.errors, []);
      assert.equal(renderer.replacementCharacters, false);
      if (selected)
        assert.ok(
          renderer.selections.every(Boolean),
          "Output cleared a held text selection",
        );
      for (const flow of main.flows) {
        assert.ok(flow.output.bytes <= OUTPUT_CREDIT_BYTES);
        assert.ok(flow.parserBytes <= flow.limits.parserBytes);
      }
      for (const queue of renderer.queues)
        assert.ok(queue.bytes <= OUTPUT_CREDIT_BYTES);
      const rss = (pid) =>
        Number(
          execFileSync("ps", ["-o", "rss=", "-p", String(pid)], {
            encoding: "utf8",
          }).trim(),
        ) * 1024;
      samples.push({
        second: tick + 1,
        elapsedMs: Date.now() - started,
        mainRssBytes: rss(main.pid),
        mainHeapUsedBytes: main.memory.heapUsed,
        mainHeapTotalBytes: main.memory.heapTotal,
        mainExternalBytes: main.memory.external,
        mainOldSpaceBytes: main.spaces.find(
          (space) => space.space_name === "old_space",
        ).space_used_size,
        mainContexts: main.heap.number_of_native_contexts,
        mainDetachedContexts: main.heap.number_of_detached_contexts,
        rendererRssBytes: rss(main.rendererPid),
        rendererHeapUsedBytes: rendererMemory.usedSize,
        cliRssBytes: main.flows.map((flow) => rss(flow.pid)),
        outputBytes: main.flows.map((flow) => flow.output.bytes),
        parserBytes: main.flows.map((flow) => flow.parserBytes),
        rendererBytes: renderer.queues.map((queue) => queue.bytes),
        deliveredBytes: renderer.deliveredBytes,
      });
      if (tick % 15 === 0)
        await fs.writeFile(samplesFile, JSON.stringify(samples, null, 2));
    }
    await fs.writeFile(samplesFile, JSON.stringify(samples, null, 2));
    await page.evaluate(() => {
      for (const entry of window.loadTerminals) entry.interactions.resume();
    });
    console.error("Electron load: workload complete");
    await sleep(500);
    await fs.mkdir(path.join(root, "artifacts"), { recursive: true });
    await page.screenshot({
      path: path.join(root, "artifacts/terminal-flow-electron.png"),
    });
    const windows = samples
      .reduce((groups, sample) => {
        const group = Math.floor((sample.second - 1) / 60);
        (groups[group] ||= []).push(sample);
        return groups;
      }, [])
      .map((group) => ({
        fromSecond: group[0].second,
        toSecond: group.at(-1).second,
        mainRssMinBytes: Math.min(
          ...group.map((sample) => sample.mainRssBytes),
        ),
        mainRssMaxBytes: Math.max(
          ...group.map((sample) => sample.mainRssBytes),
        ),
        mainHeapMinBytes: Math.min(
          ...group.map((sample) => sample.mainHeapUsedBytes),
        ),
        mainHeapMaxBytes: Math.max(
          ...group.map((sample) => sample.mainHeapUsedBytes),
        ),
        rendererRssMinBytes: Math.min(
          ...group.map((sample) => sample.rendererRssBytes),
        ),
        rendererRssMaxBytes: Math.max(
          ...group.map((sample) => sample.rendererRssBytes),
        ),
        cliRssMaxBytes: panes.map((_pane, index) =>
          Math.max(...group.map((sample) => sample.cliRssBytes[index])),
        ),
      }));
    const afterWarmup = samples.slice(
      Math.min(60, Math.floor(samples.length / 4)),
    );
    const first = afterWarmup.slice(
      0,
      Math.max(1, Math.floor(afterWarmup.length / 4)),
    );
    const last = afterWarmup.slice(
      -Math.max(1, Math.floor(afterWarmup.length / 4)),
    );
    const average = (items, key) =>
      items.reduce((sum, item) => sum + item[key], 0) / items.length;
    const drift = (key) => average(last, key) - average(first, key);
    // GC, canvas and allocator caches may retain pages; a bounded plateau is
    // measured after warmup, independently from the exact queue byte limits.
    const tolerance = 16 * 1024 * 1024;
    const gcDiagnostic = process.env.SUSHIAI_STREAM_LOAD_GC_DIAGNOSTIC
      ? await page.evaluate(() => window.bridge.loadCollect())
      : undefined;
    const measured = {
      passed:
        drift("mainRssBytes") <= tolerance &&
        drift("rendererRssBytes") <= tolerance,
      durationSeconds: seconds,
      panels: panes.length,
      consumer:
        "hidden Electron, production terminal IPC and output queue, actual xterm write acknowledgements, real text selection for 15 seconds every 30 seconds, Unicode paste",
      deliveredBytes: samples.at(-1).deliveredBytes,
      maxOutputCreditBytes: Math.max(
        ...samples.flatMap((sample) => sample.outputBytes),
      ),
      maxRendererQueueBytes: Math.max(
        ...samples.flatMap((sample) => sample.rendererBytes),
      ),
      maxParserBytes: Math.max(
        ...samples.flatMap((sample) => sample.parserBytes),
      ),
      mainRssGrowthBytes: drift("mainRssBytes"),
      rendererRssGrowthBytes: drift("rendererRssBytes"),
      mainHeapGrowthBytes: drift("mainHeapUsedBytes"),
      warmupSamples: samples.length - afterWarmup.length,
      rssDriftToleranceBytes: tolerance,
      gcDiagnostic,
      windows,
    };
    await fs.writeFile(
      path.join(root, "artifacts/terminal-flow-electron-measured.json"),
      JSON.stringify(measured, null, 2),
    );
    assert.ok(
      drift("mainRssBytes") <= tolerance,
      "Electron main RSS continued growing after warmup",
    );
    assert.ok(
      drift("rendererRssBytes") <= tolerance,
      "xterm renderer RSS continued growing after warmup",
    );
    await app.close();
    console.error("Electron load: app closed");
    app = undefined;
    for (const pane of panes) {
      const state = await request(daemon.socket, "pane.read", {
        pane_id: pane.pane,
        source: "visible",
        format: "text",
        strip_ansi: true,
      });
      assert.ok(
        state.read.text.includes("Unicode"),
        "closing the consumer must retain the live daemon pane",
      );
    }
    console.log(
      JSON.stringify(
        {
          ...measured,
          processesPreservedAfterClose: panes.length,
        },
        null,
        2,
      ),
    );
  } finally {
    await app?.close();
    for (const pane of panes)
      await request(daemon.socket, "workspace.close", {
        workspace_id: pane.workspace,
      }).catch(() => {});
    await daemon.close();
    await fs.rm(fixture, { recursive: true, force: true });
  }
}

(process.env.SUSHIAI_STREAM_LOAD_CONSUMER === "node"
  ? nodeLoad
  : electronLoad)().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
