const os = require("node:os");
const path = require("node:path");
const { existsSync, statSync } = require("node:fs");
const { spawn } = require("node:child_process");
const { quote } = require("../connections.cjs");
const { harnessFor, isHarness, chatPrompt, IMAGE } = require("../harness.cjs");

function registerChatIpc({
  handle,
  send,
  getConnections,
  agentCatalog: catalogs,
  executable,
  directory,
  id,
  chats,
}) {
  /** The daemon manager's name for a chat's endpoint. */
  const hostOf = (endpoint) =>
    endpoint?.startsWith("ssh:") ? endpoint.slice(4) : "local";
  /** The agent catalog of the host a chat runs on. An ssh chat runs over plain
   * ssh and needs no daemon there, so a host whose daemon cannot answer gets
   * this Mac's catalog of the same CLIs. */
  async function catalogFor(endpoint) {
    const host = hostOf(endpoint);
    const own = await catalogs.of(host);
    return host === "local" || Object.keys(own).length
      ? own
      : catalogs.of("local");
  }
  function stop(panelId) {
    const proc = chats.get(panelId);
    if (!proc) return;
    // A turn still waiting for the catalog has no process yet: mark it so it
    // never spawns.
    if (proc.pending) {
      proc.cancelled = true;
      return;
    }
    try {
      process.kill(-proc.pid, "SIGTERM");
    } catch {
      proc.kill();
    }
  }

  handle("chat-cancel", (panelId) => stop(id(panelId)));
  handle("chat", async (request) => {
    id(request.panelId);
    if (chats.has(request.panelId))
      throw new Error("A response is already running.");
    // The panel is taken before any await, so a second send is refused and a
    // stop during the wait can find the turn.
    const pending = { pending: true, cancelled: false };
    chats.set(request.panelId, pending);
    try {
      return await startChat(request, pending);
    } catch (error) {
      if (chats.get(request.panelId) === pending) chats.delete(request.panelId);
      throw error;
    }
  });
  async function startChat(
    { panelId, cwd, agent, messages, endpoint, model, effort, permission },
    pending,
  ) {
    const connections = getConnections();
    const remote = endpoint?.startsWith("ssh:")
      ? connections.get(endpoint)
      : null;
    if (!remote) directory(cwd);
    else if (typeof cwd !== "string" || !cwd.startsWith("/"))
      throw new Error("Choose an absolute remote project folder.");
    if (!isHarness(agent)) throw new Error("Choose Claude Code or Codex.");
    const harness = harnessFor(agent);
    const binary = remote ? "/usr/bin/ssh" : executable(agent);
    if (!binary)
      throw new Error(
        `${agent} is not installed. Install and sign in to the CLI first.`,
      );
    if (
      !Array.isArray(messages) ||
      messages.length > 200 ||
      messages.some(
        (message) =>
          !["user", "assistant"].includes(message.role) ||
          typeof message.text !== "string",
      )
    )
      throw new Error("Invalid conversation");
    if (
      messages.some(
        (message) =>
          message.attachments !== undefined &&
          (!Array.isArray(message.attachments) ||
            message.attachments.length > 20 ||
            message.attachments.some(
              (attachment) =>
                typeof attachment !== "string" ||
                attachment.length > 1000 ||
                !path.isAbsolute(attachment),
            )),
      )
    )
      throw new Error("Invalid attachments");
    const attachments = messages.flatMap(
      (message) => message.attachments || [],
    );
    if (attachments.length && remote)
      throw new Error("Attachments work with local projects only.");
    const present = attachments.filter((attachment) => existsSync(attachment));
    const dirs = [
      ...new Set(
        present.map((attachment) =>
          statSync(attachment).isDirectory()
            ? attachment
            : path.dirname(attachment),
        ),
      ),
    ].slice(0, 30);
    const images = present
      .filter((attachment) => IMAGE.test(attachment))
      .slice(-10);
    const prompt = chatPrompt(messages);
    if (prompt.length > 200000)
      throw new Error("Conversation is too long. Start a new chat.");
    // No entry (an old or offline daemon) leaves the values to the CLI after
    // a format check.
    const offered = (await catalogFor(endpoint))[agent];
    if (pending.cancelled) {
      chats.delete(panelId);
      // The renderer leaves its busy state only on `done`.
      send("chat-data", { panelId, done: true });
      return { started: false };
    }
    let args = harness.chatArgs(
      { model, effort, permission, dirs, images },
      offered,
    );
    if (remote)
      args = [
        ...connections.args(remote),
        remote.host,
        `export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; cd ${quote(cwd)} && exec ${quote(agent)} ${args.map(quote).join(" ")}`,
      ];
    const proc = spawn(binary, args, {
      cwd: remote ? os.homedir() : cwd,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
      detached: true,
    });
    chats.set(panelId, proc);
    // Codex reports no model of its own, so the composer keeps showing the
    // one it asked for, or the CLI's configured default.
    if (!harness.modelFromEvents)
      send("chat-data", {
        panelId,
        model: model || offered?.defaultModel || "",
      });
    const parse = harness.chatEvent;
    let buffered = "",
      diagnostics = "",
      warnings = "",
      fatal = "",
      full = "",
      answered = false,
      usage;
    proc.stdout.setEncoding("utf8");
    proc.stderr.setEncoding("utf8");
    const emit = (text) => {
      answered = true;
      send("chat-data", { panelId, text });
    };
    proc.stdout.on("data", (data) => {
      buffered += data;
      let boundary;
      while ((boundary = buffered.indexOf("\n")) >= 0) {
        const line = buffered.slice(0, boundary);
        buffered = buffered.slice(boundary + 1);
        let event;
        try {
          event = JSON.parse(line);
        } catch {
          continue;
        }
        const update = parse(event);
        if (!update) continue;
        if (update.model) send("chat-data", { panelId, model: update.model });
        if (update.full) full = update.full;
        if (update.usage) usage = update.usage;
        if (update.text) emit(update.text);
        else if (update.note) send("chat-data", { panelId, note: update.note });
        if (update.error) {
          if (update.fatal) fatal = update.error;
          else
            warnings =
              `${warnings ? `${warnings}\n` : ""}${update.error}`.slice(-4000);
        }
      }
    });
    proc.stderr.on("data", (data) => {
      diagnostics = (diagnostics + data).slice(-8000);
    });
    proc.on("error", (error) => {
      chats.delete(panelId);
      send("chat-data", { panelId, done: true, error: error.message });
    });
    proc.on("close", (code) => {
      chats.delete(panelId);
      if (!answered && full) emit(full);
      const failed = code
        ? diagnostics || warnings || `Agent exited (${code}).`
        : "";
      send("chat-data", {
        panelId,
        done: true,
        usage,
        error:
          fatal || failed || (answered ? undefined : warnings || undefined),
      });
    });
    proc.stdin.on("error", () => {});
    proc.stdin.end(prompt);
    return { started: true };
  }
  handle("chat-models", (endpoint) => {
    if (endpoint !== undefined && typeof endpoint !== "string")
      throw new Error("Invalid endpoint");
    return catalogFor(endpoint);
  });

  return {
    stop,
    close() {
      for (const panelId of chats.keys()) stop(panelId);
    },
  };
}

module.exports = { registerChatIpc };
