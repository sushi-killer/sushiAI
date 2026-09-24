const os = require("node:os");
const path = require("node:path");
const { existsSync, statSync } = require("node:fs");
const fs = require("node:fs/promises");
const { spawn } = require("node:child_process");
const { quote } = require("../connections.cjs");
const {
  chatArgs,
  chatPrompt,
  claudeEvent,
  codexEvent,
  IMAGE,
  ORCHESTRATOR_SERVER,
} = require("../chat-args.cjs");
const { chatModels } = require("../agent-models.cjs");

function registerChatIpc({
  handle,
  send,
  getConnections,
  executable,
  directory,
  id,
  chats,
  getOrchestrator,
}) {
  function stop(panelId) {
    const proc = chats.get(panelId);
    if (!proc) return;
    try {
      process.kill(-proc.pid, "SIGTERM");
    } catch {
      proc.kill();
    }
  }

  /** Starts the daemon if it isn't already running and writes
   * `<dataDir>/mcp-config.json` (0600, same access as `control.token` next to
   * it) fresh for this turn - `command`/`args` never change between turns,
   * only the daemon's own lifecycle does, so this is cheap to redo every time
   * rather than caching a config the daemon could have restarted under. */
  async function orchestratorMcpConfig(cwd) {
    const service = getOrchestrator?.();
    if (!service) throw new Error("The orchestrator daemon isn't available.");
    await service.ensureRunning();
    const { command, args, env } = await service.mcpServerConfig(cwd);
    const configPath = path.join(service.dataDir, "mcp-config.json");
    await fs.mkdir(service.dataDir, { recursive: true });
    await fs.writeFile(
      configPath,
      JSON.stringify({
        mcpServers: { [ORCHESTRATOR_SERVER]: { command, args, env } },
      }),
    );
    await fs.chmod(configPath, 0o600);
    return { configPath, command, args, env };
  }

  handle("chat-cancel", (panelId) => stop(id(panelId)));
  handle(
    "chat",
    async ({
      panelId,
      cwd,
      agent,
      messages,
      endpoint,
      model,
      effort,
      permission,
      orchestrator,
    }) => {
      id(panelId);
      const connections = getConnections();
      const remote = endpoint?.startsWith("ssh:")
        ? connections.get(endpoint)
        : null;
      if (!remote) directory(cwd);
      else if (typeof cwd !== "string" || !cwd.startsWith("/"))
        throw new Error("Choose an absolute remote project folder.");
      if (chats.has(panelId)) throw new Error("A response is already running.");
      if (!["claude", "codex"].includes(agent))
        throw new Error("Choose Claude Code or Codex.");
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
      const present = attachments.filter((attachment) =>
        existsSync(attachment),
      );
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
      const orchestratorMcp = orchestrator
        ? await orchestratorMcpConfig(cwd)
        : undefined;
      let args = chatArgs(agent, {
        model,
        effort,
        permission,
        dirs,
        images,
        orchestratorMcp,
        remote: Boolean(remote),
      });
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
      if (agent === "codex")
        send("chat-data", {
          panelId,
          model: model || chatModels().codex.defaultModelId || "",
        });
      const parse = agent === "claude" ? claudeEvent : codexEvent;
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
          else if (update.note)
            send("chat-data", { panelId, note: update.note });
          if (update.error) {
            if (update.fatal) fatal = update.error;
            else
              warnings =
                `${warnings ? `${warnings}\n` : ""}${update.error}`.slice(
                  -4000,
                );
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
    },
  );
  handle("chat-models", () => chatModels());

  return {
    stop,
    close() {
      for (const panelId of chats.keys()) stop(panelId);
    },
  };
}

module.exports = { registerChatIpc };
