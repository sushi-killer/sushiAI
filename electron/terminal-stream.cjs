const { spawn } = require("node:child_process");
const { randomUUID } = require("node:crypto");
const { quote } = require("./connections.cjs");
const { request } = require("./herdr.cjs");
const { createFrameDecoder } = require("./terminal-text.cjs");
const {
  MAX_FRAME_BYTES,
  createOutputDelivery,
  createInputWriter,
} = require("./terminal-flow.cjs");

function claudeForeground(info) {
  return (info?.process_info?.foreground_processes || []).some((process) =>
    [process.name, process.argv0, ...(process.argv || [])].some(
      (value) =>
        typeof value === "string" &&
        (/(?:^|\/)claude(?:$|\s)/.test(value) ||
          value.includes("/@anthropic-ai/claude-code/")),
    ),
  );
}

function scrollCommands(
  direction,
  lines,
  position,
  claude,
  steps = position.fast === true ? 3 : 1,
) {
  if (claude) {
    const report = `\x1b[<${direction === "up" ? 64 : 65};${(position.column || 0) + 1};${(position.row || 0) + 1}M`;
    return [
      {
        type: "terminal.input",
        bytes: Buffer.from(report.repeat(steps)).toString("base64"),
      },
    ];
  }
  return Array.from({ length: steps }, () => ({
    type: "terminal.scroll",
    direction,
    lines,
    source: "wheel",
    column: position.column,
    row: position.row,
  }));
}

// Process discovery is control-plane work: never put its round trip in the
// steady-state wheel path. Keep the last successful result during refreshes.
function createScrollHandler({ lookup, write, closed, now = Date.now }) {
  let foreground;
  let pending;
  let checkedAt = -Infinity;
  let remainder = 0;
  let lastDirection;
  let lastFast;
  let generation = 0;
  const refresh = () => {
    if (!pending && now() - checkedAt >= 500) {
      checkedAt = now();
      const version = generation;
      pending = Promise.resolve()
        .then(lookup)
        .then(
          (value) => {
            if (version !== generation) return;
            if (foreground !== value) remainder = 0;
            foreground = value;
          },
          () => {
            // A transient timeout must not redirect Claude wheel input into history.
          },
        )
        .finally(() => {
          if (version !== generation) return;
          pending = undefined;
          checkedAt = now();
        });
    }
    return pending;
  };
  // Warm the route while the terminal's initial frame is being attached.
  refresh();
  const scroll = async (direction, lines, position = {}) => {
    if (closed()) return;
    const lookupPending = refresh();
    if (foreground === undefined) await lookupPending;
    if (closed() || foreground === undefined) return;
    if (lastDirection !== direction || lastFast !== !!position.fast)
      remainder = 0;
    lastDirection = direction;
    lastFast = !!position.fast;
    // Mouse reports and history lines are integers. Carry tenths between wheel
    // events to deliver exactly +30%, without rounding every event up to +100%.
    remainder += position.fast ? 30 : 13;
    const amount = Math.floor(remainder / 10);
    remainder %= 10;
    for (const command of scrollCommands(
      direction,
      lines,
      position,
      foreground,
      amount,
    ))
      await write(command);
  };
  scroll.invalidate = () => {
    generation++;
    pending = undefined;
    foreground = undefined;
    checkedAt = -Infinity;
    remainder = 0;
  };
  return scroll;
}
function detectAgent(processName = "", title = "") {
  const value = `${processName} ${title}`.toLowerCase();
  return (
    ["claude", "codex", "gemini", "cursor-agent", "opencode", "aider"].find(
      (name) => new RegExp(`(?:^|[\\s/])${name}(?:$|[\\s/.:-])`).test(value),
    ) || null
  );
}
async function openHerdrStream({
  endpoint,
  panelId,
  target,
  cols,
  rows,
  connections,
  binary,
  send,
  spawnProcess = spawn,
  streamId = randomUUID(),
  preflight = (options) =>
    require("./herdr-compatibility.cjs").assertHerdrCompatibility(options),
}) {
  const compatibility = await preflight({ endpoint, connections, binary });
  binary = compatibility.cli.binary;
  const args = [
    "terminal",
    "session",
    "control",
    target,
    "--cols",
    String(cols),
    "--rows",
    String(rows),
  ];
  let command = binary,
    argv = args,
    env = { ...process.env, HERDR_SOCKET_PATH: endpoint };
  if (endpoint.startsWith("ssh:")) {
    await connections.socket(endpoint);
    const profile = connections.get(endpoint);
    const info = await connections.inspect(endpoint, {
      operation: "home",
      socket: profile.socket,
    });
    command = connections.ssh || "/usr/bin/ssh";
    argv = [
      ...connections.args(profile),
      profile.host,
      `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; HERDR_SOCKET_PATH=${quote(info.socket)} exec ${quote(binary)} ${args.map(quote).join(" ")}`,
    ];
    env = process.env;
  }
  if (!command) throw new Error("Install Herdr to attach a terminal stream.");
  const child = spawnProcess(command, argv, {
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let resolveClosed;
  const closed = new Promise((resolve) => {
    resolveClosed = resolve;
  });
  const entry = {
    history: "",
    exited: false,
    source: "herdr",
    target,
    streamId,
  };
  const decodeFrame = createFrameDecoder(MAX_FRAME_BYTES);
  const maxLineBytes = Math.ceil(MAX_FRAME_BYTES / 3) * 4 + 1024;
  let buffer = Buffer.alloc(0),
    diagnostic = "",
    closing = false,
    failed = false,
    firstFrame = true,
    reportedDisconnect = false;
  const input = createInputWriter(child.stdin);
  const write = (value) => {
    if (entry.exited || closing)
      return Promise.reject(new Error("Terminal stream is disconnected."));
    return input.write(value);
  };
  const fail = (message) => {
    if (failed || closing) return;
    failed = true;
    diagnostic = String(message).slice(0, 4000);
    buffer = Buffer.alloc(0);
    child.stdout.destroy();
    delivery.close();
    input.close(new Error(diagnostic));
    child.kill();
  };
  const delivery = createOutputDelivery({
    streamId,
    send: (event) => send("terminal-data", { panelId, ...event }),
    changed: () => {
      if (!failed && !closing) consume();
    },
  });
  function consume() {
    try {
      while (!delivery.blocked && !failed && !closing) {
        const boundary = buffer.indexOf(10);
        if (boundary < 0) {
          if (buffer.length > maxLineBytes)
            fail(
              "Terminal frame exceeds the supported size. Reconnect to refresh the screen.",
            );
          break;
        }
        if (boundary > maxLineBytes)
          throw new Error("Terminal frame exceeds the supported size.");
        const line = buffer.subarray(0, boundary).toString("utf8");
        buffer = buffer.subarray(boundary + 1);
        const frame = JSON.parse(line);
        if (frame.type === "terminal.frame") {
          if (
            typeof frame.full !== "boolean" ||
            typeof frame.bytes !== "string"
          )
            throw new Error("Malformed Herdr terminal frame.");
          if (frame.bytes.length > Math.ceil(MAX_FRAME_BYTES / 3) * 4)
            throw new Error("Terminal frame exceeds the supported size.");
          if (firstFrame && !frame.full)
            throw new Error(
              "Herdr did not provide an initial terminal snapshot.",
            );
          const data = decodeFrame(frame.bytes, frame.full);
          firstFrame = false;
          if (data || frame.full) delivery.enqueue(data, frame.full);
        } else if (frame.type === "terminal.closed") {
          fail(
            typeof frame.reason === "string"
              ? frame.reason
              : "Terminal detached. Reconnect to continue.",
          );
        } else throw new Error("Unsupported Herdr terminal stream.");
      }
      if (delivery.blocked || failed || closing) child.stdout.pause();
      else child.stdout.resume();
    } catch (error) {
      fail(`${error.message} Reconnect to refresh the terminal.`);
    }
  }
  const scroll = createScrollHandler({
    lookup: () =>
      connections
        .socket(endpoint)
        .then((socket) =>
          request(socket, "pane.process_info", { pane_id: target }, 500),
        )
        .then(claudeForeground),
    write,
    closed: () => closing || entry.exited,
  });
  entry.proc = {
    write: (data) => {
      // Enter and process-control keys can launch/exit the foreground program.
      if (/[\r\n\x03\x04\x1a]/.test(data)) scroll.invalidate();
      return write({
        type: "terminal.input",
        bytes: Buffer.from(data).toString("base64"),
      });
    },
    resize: (cols, rows) => write({ type: "terminal.resize", cols, rows }),
    kill: () => {
      if (closing) return closed;
      closing = true;
      delivery.close();
      buffer = Buffer.alloc(0);
      // Paused pipes hold ChildProcess.close until their unread bytes drain.
      child.stdout.resume();
      input
        .write({ type: "terminal.release" })
        .catch(() => {})
        .finally(() => child.stdin.end());
      const timer = setTimeout(() => child.kill(), 500);
      timer.unref();
      return closed;
    },
    scroll,
  };
  entry.ack = (token, sequence) => delivery.ack(token, sequence);
  entry.flowStats = () => ({
    output: delivery.stats,
    input: input.stats,
    parserBytes: buffer.length,
    pid: child.pid,
    limits: { frameBytes: MAX_FRAME_BYTES, parserBytes: maxLineBytes + 65536 },
  });
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    if (closing || failed) return;
    if (buffer.length + chunk.length > maxLineBytes + 65536)
      return fail(
        "Terminal frame exceeds the supported size. Reconnect to refresh the screen.",
      );
    buffer = Buffer.concat([buffer, chunk]);
    consume();
  });
  child.stderr.on("data", (data) => {
    diagnostic = (diagnostic + data).slice(-4000);
  });
  child.stdin.on("error", () => {});
  child.on("error", (error) => {
    diagnostic = error.message;
  });
  const reportDisconnect = (code) => {
    if (reportedDisconnect) return;
    reportedDisconnect = true;
    entry.exited = true;
    input.close(new Error(diagnostic || "Terminal stream is disconnected."));
    if (closing || failed) {
      buffer = Buffer.alloc(0);
      delivery.close();
      child.stdout.destroy();
    }
    send("terminal-data", {
      panelId,
      streamId,
      exitCode: code || 0,
      data: "",
      error: closing
        ? undefined
        : diagnostic || "Terminal detached. Reconnect to continue.",
    });
  };
  child.on("exit", reportDisconnect);
  child.on("close", (code) => {
    reportDisconnect(code);
    resolveClosed();
  });
  return entry;
}
module.exports = {
  openHerdrStream,
  detectAgent,
  claudeForeground,
  scrollCommands,
  createScrollHandler,
};
