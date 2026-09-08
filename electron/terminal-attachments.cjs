const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { request } = require("./herdr.cjs");
const { detectAgent } = require("./terminal-stream.cjs");

async function attachmentAgent(terminal, connections) {
  if (terminal.source === "herdr") {
    const socket = await connections.socket(terminal.endpoint);
    const info = await request(socket, "pane.process_info", {
      pane_id: terminal.target,
    });
    return (info?.process_info?.foreground_processes || []).some((process) =>
      [process.name, process.argv0, ...(process.argv || [])].some(
        (value) => typeof value === "string" && !!detectAgent(value),
      ),
    );
  }
  return !!detectAgent(
    terminal.remote ? terminal.command : terminal.proc?.process,
  );
}

async function storeTerminalAttachment({
  terminal,
  name,
  data,
  dataDir,
  connections,
}) {
  if (!terminal || terminal.exited)
    throw new Error("This terminal is not running.");
  if (!(await attachmentAgent(terminal, connections)))
    throw new Error("Images can only be attached to an active agent session.");
  if (typeof data !== "string" || data.length > 28 * 1024 * 1024)
    throw new Error("Attach files up to 20 MB.");
  const bytes = Buffer.from(data, "base64");
  if (!bytes.length || bytes.length > 20 * 1024 * 1024)
    throw new Error("Choose non-empty files up to 20 MB.");
  const safe =
    path
      .basename(typeof name === "string" ? name : "")
      .replace(/[^\w.\- ]+/g, "_")
      .slice(-80) || "pasted";
  if (terminal.endpoint?.startsWith("ssh:")) {
    const result = await connections.inspect(terminal.endpoint, {
      operation: "terminal_attachment",
      name: safe,
      data: bytes.toString("base64"),
    });
    if (typeof result.path !== "string" || !result.path.startsWith("/"))
      throw new Error("Remote attachment returned an invalid path.");
    return result.path;
  }
  const directory = path.join(dataDir, "attachments");
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  const file = path.join(directory, `${randomUUID()}-${safe}`);
  await fs.writeFile(file, bytes, { flag: "wx", mode: 0o600 });
  return file;
}
module.exports = { storeTerminalAttachment };
