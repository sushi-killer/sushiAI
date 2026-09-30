import type {
  ChatAction,
  ChatToolConfig,
  ChatToolRow,
  ChatToolServer,
} from "./types";

/** The line under a connected tool's name in Settings: what it gives the
 * chat, or why it gives nothing. */
export function toolStatusLine(
  config: ChatToolConfig,
  row: ChatToolRow | undefined,
  codexChat = false,
): string {
  if (!config.enabled) return "Off";
  if (!row || row.status === "unchecked") return "Not checked yet";
  if (row.status !== "ok")
    return row.reason || "Not available: the chat runs without it";
  const reads = row.tools.filter((t) => t.kind === "read").length;
  const writes = row.tools.length - reads;
  const parts = [`${reads} read`, `${writes} ask first`];
  const line = `Connected · ${parts.join(" · ")}`;
  return codexChat && row.codex ? `${line} · ${row.codex}` : line;
}

/** A tool id for a server the owner adds: its label as a slug, made unique. */
export function newToolId(label: string, taken: string[]): string {
  const base =
    label
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "tool";
  let id = base;
  for (let n = 2; taken.includes(id); n += 1) id = `${base}-${n}`;
  return id;
}

/** The connected tool a chosen server of the owner's MCP config becomes: on,
 * and only a reference, so nothing secret is copied. */
export function toolFromServer(
  server: ChatToolServer,
  taken: string[],
): ChatToolConfig {
  return {
    id: newToolId(server.label, taken),
    label: server.label,
    enabled: true,
    server: { ref: server.ref },
  };
}

/** The owner's read/write call for one tool: choosing what orchd would have
 * guessed anyway clears the override. */
export function withOverride(
  config: ChatToolConfig,
  tool: string,
  kind: "read" | "write",
  guessed: "read" | "write",
): ChatToolConfig {
  const overrides = { ...config.overrides };
  if (kind === guessed) delete overrides[tool];
  else overrides[tool] = kind;
  const next = { ...config };
  delete next.overrides;
  return Object.keys(overrides).length ? { ...next, overrides } : next;
}

/** What the ToolConfirm card shows of a call's arguments. */
export function payloadPreview(args: Record<string, unknown>): string {
  return JSON.stringify(args, null, 2);
}

/** The owner's edited arguments, or why they are not usable. */
export function parseArgsText(
  text: string,
): { args: Record<string, unknown> } | { error: string } {
  try {
    const value: unknown = JSON.parse(text);
    if (value && typeof value === "object" && !Array.isArray(value))
      return { args: value as Record<string, unknown> };
    return { error: "The arguments must be a JSON object." };
  } catch {
    return { error: "That is not valid JSON." };
  }
}

/** The state a finished card shows, or null while it still asks. */
export function actionOutcome(action: ChatAction): string | null {
  switch (action.state) {
    case "sending":
      return "Sending…";
    case "sent":
      return "Sent";
    case "failed":
      return action.error ? `Failed: ${action.error}` : "Failed";
    case "declined":
      return "Not sent";
    default:
      return null;
  }
}
