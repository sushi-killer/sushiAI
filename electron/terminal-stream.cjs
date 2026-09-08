const { spawn } = require("node:child_process");
const { quote } = require("./connections.cjs");
const { request } = require("./herdr.cjs");
const { createFrameDecoder } = require("./terminal-text.cjs");

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
      write(command);
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
}) {
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
    command = "/usr/bin/ssh";
    argv = [
      ...connections.args(profile),
      profile.host,
      `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; HERDR_SOCKET_PATH=${quote(info.socket)} exec herdr ${args.map(quote).join(" ")}`,
    ];
    env = process.env;
  }
  if (!command) throw new Error("Install Herdr to attach a terminal stream.");
  const child = spawn(command, argv, { env, stdio: ["pipe", "pipe", "pipe"] });
  const entry = { history: "", exited: false, source: "herdr", target };
  const decodeFrame = createFrameDecoder();
  let buffer = "",
    diagnostic = "",
    closing = false;
  const write = (value) => {
    if (!entry.exited && !child.stdin.destroyed)
      child.stdin.write(JSON.stringify(value) + "\n");
  };
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
      write({
        type: "terminal.input",
        bytes: Buffer.from(data).toString("base64"),
      });
    },
    resize: (cols, rows) => write({ type: "terminal.resize", cols, rows }),
    kill: () => {
      closing = true;
      write({ type: "terminal.release" });
      child.stdin.end();
      const timer = setTimeout(() => child.kill(), 500);
      timer.unref();
    },
    scroll,
  };
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    if (buffer.length > 24 * 1024 * 1024) {
      diagnostic = "Terminal frame too large";
      return child.kill();
    }
    let boundary;
    while ((boundary = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 1);
      try {
        const frame = JSON.parse(line);
        if (frame.type === "terminal.frame") {
          const data = decodeFrame(frame.bytes, frame.full);
          entry.history = frame.full
            ? data
            : (entry.history + data).slice(-2 * 1024 * 1024);
          send("terminal-data", { panelId, data });
        } else if (frame.type === "terminal.closed")
          diagnostic = frame.reason || "Terminal detached";
      } catch {
        diagnostic = "Unsupported Herdr terminal stream";
      }
    }
  });
  child.stderr.on("data", (data) => {
    diagnostic = (diagnostic + data).slice(-4000);
  });
  child.stdin.on("error", () => {});
  child.on("error", (error) => {
    diagnostic = error.message;
  });
  child.on("close", (code) => {
    entry.exited = true;
    send("terminal-data", {
      panelId,
      exitCode: code || 0,
      data: closing
        ? ""
        : `\r\n\x1b[90m${diagnostic || "Terminal detached. Reconnect to continue."}\x1b[0m\r\n`,
    });
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
