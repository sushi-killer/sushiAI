const { spawn } = require("node:child_process");
const { quote } = require("./connections.cjs");
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
  const entry = { history: "", exited: false, source: "herdr" };
  let buffer = "",
    diagnostic = "",
    closing = false;
  const write = (value) => {
    if (!entry.exited && !child.stdin.destroyed)
      child.stdin.write(JSON.stringify(value) + "\n");
  };
  entry.proc = {
    write: (data) =>
      write({
        type: "terminal.input",
        bytes: Buffer.from(data).toString("base64"),
      }),
    resize: (cols, rows) => write({ type: "terminal.resize", cols, rows }),
    kill: () => {
      closing = true;
      write({ type: "terminal.release" });
      child.stdin.end();
      const timer = setTimeout(() => child.kill(), 500);
      timer.unref();
    },
    scroll: (direction, lines) =>
      write({ type: "terminal.scroll", direction, lines, source: "wheel" }),
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
          const data = Buffer.from(frame.bytes, "base64").toString("utf8");
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
module.exports = { openHerdrStream, detectAgent };
