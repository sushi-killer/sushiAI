"use strict";

// The one place that knows how each agent CLI behaves. A harness is one object
// in HARNESSES with the same interface; adding a harness means adding one
// object here. Callers use harnessFor(name) and never branch on a name.
//
//   name                     the agent id
//   login                    argv words of the harness's own login
//   modelFromEvents          true when chat events report the resolved model
//   chatArgs(options, agent) argv of a one-shot chat turn (validated)
//   chatEvent(event)         one stream event as a chat update, or null
//   mcpArgs(server)          flags that attach an MCP server
//   headlessTurnArgs(opts)   flags of one headless builder turn
//   parseTurn(screenText)    {state: running|done|failed, text, session}
//
// What a chat may ask for - the models, their reasoning levels and the
// permission modes - is not here: it is the agent's catalog entry, which the
// daemon serves as `agents.catalog`. `chatArgs` takes that entry and refuses
// anything outside it. Values come from the renderer, so each is validated
// before it reaches a process (locally argv, remotely a quoted ssh line).

/** `agent` is one entry of `agents.catalog` (its `efforts` and `permissions`).
 * A catalog that has not arrived yet has neither, so nothing is accepted: an
 * empty list refuses rather than passes. */
function checkChatOptions(agent, { model, effort, permission }) {
  if (model && !/^[\w.:-]{1,64}$/.test(model)) throw new Error("Invalid model");
  // Each CLI publishes its own reasoning levels; Codex adds "ultra", Claude does not.
  if (effort && !(agent?.efforts || []).includes(effort))
    throw new Error("Invalid effort");
  if (permission && !(agent?.permissions || []).includes(permission))
    throw new Error("Invalid permission mode");
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
 * The JSON objects a terminal screen shows, in order. An object starts a row
 * with `{"` (Claude does not put `type` first; inside a JSON string quotes are
 * escaped, so a wrapped row seldom looks like a start). A row the terminal
 * wrapped is joined with the next rows until it parses; when a continuation
 * row also looks like a start, the earlier start keeps growing alongside it.
 * Other rows (banners, log noise) are skipped.
 */
function screenObjects(raw) {
  const objects = [];
  let buffers = [];
  for (const row of String(raw ?? "").split(/\r?\n/)) {
    if (row.startsWith('{"')) buffers = [...buffers.slice(-7), ""];
    if (!buffers.length) continue;
    buffers = buffers.map((buffer) => buffer + row);
    const at = buffers.findIndex((buffer) => {
      try {
        objects.push(JSON.parse(buffer));
        return true;
      } catch {
        return false;
      }
    });
    if (at >= 0) buffers = [];
  }
  return objects;
}

const RUNNING = Object.freeze({ state: "running", text: "", session: "" });

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

const claude = {
  name: "claude",
  login: ["claude", "/login"],
  modelFromEvents: true,
  chatArgs(options = {}, agent) {
    checkChatOptions(agent, options);
    const { model, effort, permission, dirs = [] } = options;
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
    if (permission && permission !== "default")
      args.push("--permission-mode", permission);
    // Variadic, so it stays last; the prompt arrives on stdin, not argv.
    if (dirs.length) args.push("--add-dir", ...dirs);
    return args;
  },
  chatEvent: claudeEvent,
  mcpArgs({ name, command, args }) {
    return [
      "--mcp-config",
      JSON.stringify({ mcpServers: { [name]: { command, args } } }),
    ];
  },
  /** `mcp` is {name, command, args, tools} or null. The tools are an allowlist. */
  headlessTurnArgs({ effort, mcp = null }) {
    const tools = [
      // Only what the builder skill needs: read the catalog, then validate,
      // save and dry-run a draft. Never `register`, `grant`, `approve`, a run
      // or the run-only `ask` and `propose`.
      "Bash(sushiai routines help:*)",
      "Bash(sushiai routines show:*)",
      "Bash(sushiai routines --json:*)",
      "Bash(sushiai routines)",
      "Bash(sushiai projects:*)",
      "Bash(sushiai hosts:*)",
      "Bash(sushiai routine validate:*)",
      "Bash(sushiai routine save:*)",
      "Bash(sushiai routine dry-run:*)",
      "Read",
      "Grep",
      "Glob",
      // Relative to the builder's own folder: the routine folder it creates
      // there, never an absolute or outside path.
      "Edit(./**)",
    ];
    if (mcp)
      tools.push(...mcp.tools.map((tool) => `mcp__${mcp.name}__${tool}`));
    // A headless session (set on session.create) prints one JSON result per
    // turn: it skips the folder trust dialog and needs no screen scraping. The
    // owner's next message is the next turn (`resume`), so the builder cannot
    // wait for an answer inside a turn.
    return [
      "--output-format",
      "json",
      "--effort",
      effort,
      "--allowedTools",
      tools.join(" "),
      // Deny what the list does not name, whatever default mode the owner's
      // own settings carry (an "auto" default would let any Bash through).
      "--permission-mode",
      "dontAsk",
      ...(mcp ? this.mcpArgs(mcp) : []),
    ];
  },
  /** One JSON result object; its `result` is the answer. */
  parseTurn(raw) {
    const done = screenObjects(raw).find(
      (value) => value && typeof value.result === "string",
    );
    if (!done) return RUNNING;
    return {
      state: done.is_error ? "failed" : "done",
      text: done.result,
      session: typeof done.session_id === "string" ? done.session_id : "",
    };
  },
};
const codex = {
  name: "codex",
  login: ["codex", "login"],
  modelFromEvents: false,
  chatArgs(options = {}, agent) {
    checkChatOptions(agent, options);
    const { model, effort, permission, images = [] } = options;
    // ponytail: codex has no interactive approvals in exec; map modes onto its sandbox.
    // Its read-only sandbox already reads the whole disk, so attachments need no --add-dir.
    const args = [
      "exec",
      ...images.flatMap((file) => ["--image", file]),
      "--json",
      "--skip-git-repo-check",
    ];
    if (model) args.push("--model", model);
    if (effort) args.push("-c", `model_reasoning_effort="${effort}"`);
    if (permission === "bypassPermissions")
      args.push("--dangerously-bypass-approvals-and-sandbox");
    else if (permission === "acceptEdits")
      args.push("--sandbox", "workspace-write");
    return [...args, "-"];
  },
  chatEvent: codexEvent,
  mcpArgs({ name, command, args }) {
    return [
      "-c",
      `mcp_servers.${name}.command=${JSON.stringify(command)}`,
      "-c",
      `mcp_servers.${name}.args=${JSON.stringify(args)}`,
    ];
  },
  // `codex exec resume` has no `-s`, so the sandbox is a config value. The
  // builder's folder is its working root; there is no network access and
  // nothing else is writable (not /tmp, not $TMPDIR). `mcp_servers={}` drops
  // every server of the owner's config, so only the one passed here returns.
  headlessTurnArgs({ effort, mcp = null }) {
    return [
      // `-c mcp_servers={}` only merges into the owner's table, so the config
      // itself is not loaded (auth still comes from CODEX_HOME).
      "--ignore-user-config",
      "-c",
      'sandbox_mode="workspace-write"',
      "-c",
      "sandbox_workspace_write.network_access=false",
      "-c",
      "sandbox_workspace_write.exclude_slash_tmp=true",
      "-c",
      "sandbox_workspace_write.exclude_tmpdir_env_var=true",
      "-c",
      `model_reasoning_effort=${effort}`,
      "-c",
      "mcp_servers={}",
      ...(mcp ? this.mcpArgs(mcp) : []),
      // Nobody answers an approval in `codex exec`: the allowlisted tools are
      // approved up front, every other tool stays refused.
      ...(mcp?.tools ?? []).flatMap((tool) => [
        "-c",
        `mcp_servers.${mcp.name}.tools.${tool}.approval_mode="approve"`,
      ]),
    ];
  },
  /** JSON lines. The turn ends with `turn.completed` (the last `agent_message`
   * is the answer) or `turn.failed`; `thread.started` carries the session. */
  parseTurn(raw) {
    let session = "";
    let text = null;
    for (const event of screenObjects(raw)) {
      if (event.type === "thread.started") session = event.thread_id ?? "";
      else if (
        event.type === "item.completed" &&
        event.item?.type === "agent_message"
      )
        text = String(event.item.text ?? "");
      else if (event.type === "turn.completed" && text !== null)
        return { state: "done", text, session };
      else if (event.type === "turn.failed")
        return {
          state: "failed",
          text: String(event.error?.message ?? "The turn failed."),
          session,
        };
    }
    return RUNNING;
  },
};

const HARNESSES = { claude, codex };

/** The harness object for a name; an unknown name is an error. */
function harnessFor(name) {
  if (!Object.hasOwn(HARNESSES, name))
    throw new Error(`Unknown harness ${name}`);
  return HARNESSES[name];
}

const isHarness = (name) => Object.hasOwn(HARNESSES, name);

module.exports = {
  harnessFor,
  isHarness,
  screenObjects,
  chatPrompt,
  IMAGE,
};
