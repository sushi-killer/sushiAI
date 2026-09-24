// CLI plumbing for a one-shot chat turn: the argv each agent gets, the prompt
// it reads on stdin, and how Codex's JSON event stream maps to chat updates.
// Values come from the renderer, so each is validated before it reaches a
// process (locally argv, remotely a quoted ssh line).
const { CLAUDE_EFFORTS, CODEX_EFFORTS } = require("./agent-models.cjs");
const PERMISSIONS = ["default", "acceptEdits", "plan", "bypassPermissions"];
const EFFORTS = { claude: CLAUDE_EFFORTS, codex: CODEX_EFFORTS };
// The orchestrator MCP server name (matches `sushiai-orchestrator` in the
// `--mcp-config`/`-c mcp_servers...` value chat.cjs builds) and the 8 tools
// it exposes - `task_delete`/`settings_set`/`secrets_*` are deliberately not
// in this list; see `orchd/src/mcp.rs`'s own comment for why not.
const ORCHESTRATOR_SERVER = "sushiai-orchestrator";
const ORCHESTRATOR_TOOLS = [
  "task_list",
  "task_get",
  "task_create",
  "task_start",
  "task_stop",
  "task_answer",
  "task_preflight",
  "settings_get",
];
const ORCHESTRATOR_ROLE =
  "You are the owner's task orchestrator in sushiAI. You never change files or run commands yourself: every piece of work becomes an orchd task through the sushiai-orchestrator tools, which run it in an isolated worktree, verify it and report back. Follow that server's instructions. You are not woken up between turns, so never promise to watch or report later: say the task is in the Orchestrator panel. Reply in the owner's language, briefly.";
function chatArgs(
  agent,
  {
    model,
    effort,
    permission,
    dirs = [],
    images = [],
    orchestratorMcp,
    remote,
  } = {},
) {
  if (model && !/^[\w.:-]{1,64}$/.test(model)) throw new Error("Invalid model");
  // Each CLI publishes its own reasoning levels; Codex adds "ultra", Claude does not.
  if (effort && !(EFFORTS[agent] || []).includes(effort))
    throw new Error("Invalid effort");
  if (permission && !PERMISSIONS.includes(permission))
    throw new Error("Invalid permission mode");
  // The orchestrator agent only ever reaches the daemon over its local unix
  // socket - a remote turn has no way to reach it, so refuse rather than
  // silently drop the tools.
  if (orchestratorMcp && remote)
    throw new Error("The orchestrator agent only runs on this machine.");
  if (agent === "claude") {
    // Streamed JSON instead of plain text: it carries tool activity, the model
    // Claude actually resolved to, real token counts and its context window.
    const args = [
      "--print",
      "--output-format",
      "stream-json",
      "--verbose",
      "--include-partial-messages",
    ];
    if (model) args.push("--model", model);
    if (effort) args.push("--effort", effort);
    if (permission && permission !== "default" && !orchestratorMcp)
      args.push("--permission-mode", permission);
    if (orchestratorMcp) {
      // Read-only built-ins plus the orchd tools: with Edit/Bash available the
      // agent fixes things itself instead of filing a task. `--allowedTools`
      // is variadic, so it stays before `--add-dir`.
      args.push(
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--tools",
        "Read,Grep,Glob",
        "--append-system-prompt",
        ORCHESTRATOR_ROLE,
        "--strict-mcp-config",
        "--mcp-config",
        orchestratorMcp.configPath,
        "--allowedTools",
        ...ORCHESTRATOR_TOOLS.map(
          (tool) => `mcp__${ORCHESTRATOR_SERVER}__${tool}`,
        ),
      );
    }
    // Variadic, so it stays last; the prompt arrives on stdin, not argv.
    if (dirs.length) args.push("--add-dir", ...dirs);
    return args;
  }
  // ponytail: codex has no interactive approvals in exec; map modes onto its sandbox.
  // Its read-only sandbox already reads the whole disk, so attachments need no --add-dir.
  const args = [
    "exec",
    ...images.flatMap((file) => ["--image", file]),
    "--json",
    "--skip-git-repo-check",
  ];
  if (orchestratorMcp) {
    // Codex has no tool allowlist: a read-only sandbox keeps it from doing the
    // work itself, and the role rides in as developer instructions.
    args.push(
      "--sandbox",
      "read-only",
      "-c",
      `developer_instructions=${JSON.stringify(ORCHESTRATOR_ROLE)}`,
      "-c",
      `mcp_servers.${ORCHESTRATOR_SERVER}.command=${JSON.stringify(orchestratorMcp.command)}`,
      "-c",
      `mcp_servers.${ORCHESTRATOR_SERVER}.args=${JSON.stringify(orchestratorMcp.args)}`,
      ...Object.entries(orchestratorMcp.env ?? {}).flatMap(([key, value]) => [
        "-c",
        `mcp_servers.${ORCHESTRATOR_SERVER}.env.${key}=${JSON.stringify(value)}`,
      ]),
    );
  }
  if (model) args.push("--model", model);
  if (effort) args.push("-c", `model_reasoning_effort="${effort}"`);
  if (orchestratorMcp) return [...args, "-"];
  if (permission === "bypassPermissions")
    args.push("--dangerously-bypass-approvals-and-sandbox");
  else if (permission === "acceptEdits")
    args.push("--sandbox", "workspace-write");
  return [...args, "-"];
}
/** One prompt per CLI turn. Attachments are paths the agent reads with its own tools. */
function chatPrompt(messages) {
  return messages
    .map(
      (m) =>
        `${m.role}: ${m.text}${
          m.attachments?.length
            ? `\n[Attached files and folders, read them as needed: ${m.attachments.join(", ")}]`
            : ""
        }`,
    )
    .join("\n\n");
}
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;
const short = (value, max = 64) => {
  const line = String(value ?? "")
    .replace(/[*`_#]/g, "")
    .split("\n")
    .map((row) => row.trim())
    .find(Boolean);
  if (!line) return "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
};
/**
 * One Codex `exec --json` event as a chat update, or null when it says nothing
 * a reader needs. Item types: error, agent_message, reasoning, command_execution,
 * file_change, mcp_tool_call, web_search, todo_list.
 */
function codexEvent(event) {
  if (event?.type === "turn.failed")
    return {
      error: event.error?.message || "Agent request failed.",
      fatal: true,
    };
  if (event?.type === "turn.completed" && event.usage)
    return {
      usage: {
        input: Number(event.usage.input_tokens) || 0,
        output: Number(event.usage.output_tokens) || 0,
        cached: Number(event.usage.cached_input_tokens) || 0,
      },
    };
  if (!String(event?.type || "").startsWith("item.")) return null;
  const item = event.item || {};
  const done = event.type === "item.completed";
  switch (item.type) {
    case "agent_message":
      return done && item.text ? { text: `${item.text}\n` } : null;
    // A deprecation notice arrives as an error item on a run that then succeeds,
    // so item errors are non-fatal and only surface when nothing else came back.
    case "error":
      return item.message ? { error: item.message, fatal: false } : null;
    case "reasoning":
      return { note: short(item.text) || "Thinking…" };
    case "command_execution":
      return {
        note: `${done ? "Ran" : "Running"} ${short(item.command, 52) || "a command"}`,
      };
    case "file_change": {
      const count = item.changes?.length || 0;
      return {
        note: count
          ? `Edited ${count} file${count > 1 ? "s" : ""}`
          : "Editing files",
      };
    }
    case "mcp_tool_call":
      return {
        note: `Tool ${short([item.server, item.tool].filter(Boolean).join(".")) || "call"}`,
      };
    case "web_search":
      return { note: `Searched ${short(item.query, 48) || "the web"}` };
    case "todo_list":
      return { note: "Updating the plan" };
    default:
      return null;
  }
}
/**
 * One Claude `--output-format stream-json` event as a chat update. Hook, status,
 * rate-limit and echoed assistant messages say nothing the thread needs; answer
 * text arrives as deltas, and the closing result carries usage and the window.
 */
function claudeEvent(event) {
  if (event?.type === "stream_event") {
    const inner = event.event || {};
    if (inner.type === "content_block_delta") {
      const delta = inner.delta || {};
      if (delta.type === "text_delta" && delta.text)
        return { text: delta.text };
      if (delta.type === "thinking_delta") return { note: "Thinking" };
      return null;
    }
    if (inner.type === "content_block_start") {
      const block = inner.content_block || {};
      if (block.type === "tool_use")
        return { note: `Tool ${short(block.name, 40) || "call"}` };
      if (block.type === "thinking") return { note: "Thinking" };
      return null;
    }
    if (inner.type === "message_start" && inner.message?.model)
      return { model: inner.message.model };
    return null;
  }
  if (event?.type === "system" && event.subtype === "init" && event.model)
    return { model: event.model };
  if (event?.type !== "result") return null;
  if (event.is_error)
    return {
      error: short(event.result, 400) || "Agent request failed.",
      fatal: true,
    };
  const raw = event.usage || {};
  const cached = Number(raw.cache_read_input_tokens) || 0;
  const rows = Object.entries(event.modelUsage || {});
  return {
    // A full turn's context is the prompt Claude sent, cached parts included.
    usage: {
      input:
        (Number(raw.input_tokens) || 0) +
        (Number(raw.cache_creation_input_tokens) || 0) +
        cached,
      output: Number(raw.output_tokens) || 0,
      cached,
      context: Math.max(
        0,
        ...rows.map(([, m]) => Number(m?.contextWindow) || 0),
      ),
    },
    model: rows[0]?.[0] || "",
    // Kept as a safety net: used only when no delta text reached the thread.
    full: typeof event.result === "string" ? event.result : "",
  };
}
module.exports = {
  chatArgs,
  chatPrompt,
  claudeEvent,
  codexEvent,
  EFFORTS,
  PERMISSIONS,
  IMAGE,
  ORCHESTRATOR_SERVER,
  ORCHESTRATOR_TOOLS,
};
