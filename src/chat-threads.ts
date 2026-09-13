import { uid } from "./layout.ts";
import type { ChatEvent, Message, Panel, Workspace } from "./types";

export const DEFAULT_TITLES = new Set(["Thread", "New thread"]);

/** Thread title from the first prompt: first line, trimmed to 44 chars. */
export function titleFrom(text: string): string {
  const line = text.trim().split("\n")[0].replace(/\s+/g, " ");
  return line.length > 44 ? line.slice(0, 43).trimEnd() + "…" : line;
}

export function relativeTime(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const s = Math.max(0, now - at) / 1000;
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 7) return `${Math.floor(s / 86400)}d`;
  return new Date(at).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

export type Thread = { workspace: Workspace; panel: Panel };
const byRecent = (a: Thread, b: Thread) =>
  (b.panel.updatedAt || 0) - (a.panel.updatedAt || 0);

/** Pinned threads first, then a group per project that has threads, newest on top. */
export function groupThreads(workspaces: Workspace[]) {
  const all = workspaces.flatMap((workspace) =>
    workspace.panels
      .filter((panel) => panel.kind === "chat")
      .map((panel) => ({ workspace, panel })),
  );
  return {
    pinned: all.filter((t) => t.panel.pinned).sort(byRecent),
    groups: workspaces
      .map((workspace) => ({
        workspace,
        threads: all
          .filter((t) => t.workspace.id === workspace.id && !t.panel.pinned)
          .sort(byRecent),
      }))
      .filter((group) => group.threads.length > 0),
  };
}

/**
 * Context in use for the header. Codex reports real token counts; Claude's text
 * output does not, so its conversation is estimated at ~4 chars per token.
 */
export function contextUsage(
  messages: { text: string }[] | undefined,
  limit = 200_000,
  counted?: number,
): string {
  const tokens =
    counted ??
    Math.round((messages || []).reduce((n, m) => n + m.text.length, 0) / 4);
  if (!tokens) return "";
  const size = tokens >= 1000 ? `${(tokens / 1000).toFixed(1)}k` : `${tokens}`;
  return `${size} · ${Math.max(1, Math.round((tokens / limit) * 100))}%`;
}

/**
 * A resolved model id as a person reads it. Claude answers with ids like
 * `claude-opus-5[1m]`; Codex slugs already have a label in the catalogue.
 */
export function modelName(id: string): string {
  if (!id.startsWith("claude-")) return id.replace(/-\d{8}$/, "");
  const long = /\[1m\]/i.test(id);
  const [family, ...rest] = id
    .replace(/\[1m\]/i, "")
    .replace(/-\d{8}$/, "")
    .replace(/^claude-/, "")
    .split("-");
  const name = [
    family.charAt(0).toUpperCase() + family.slice(1),
    rest.join("."),
  ]
    .filter(Boolean)
    .join(" ");
  return long ? `${name} · 1M` : name;
}

/** Opens a user turn: appends the prompt, parks an empty assistant message for
 * the stream to fill, and names an untitled thread after the first prompt. */
export function startUserTurn(
  panel: Panel,
  text: string,
  attachments: string[] = [],
  now = Date.now(),
): { messages: Message[]; update: Partial<Panel> } {
  const messages: Message[] = [
    ...(panel.messages || []),
    {
      id: uid(),
      role: "user",
      text,
      attachments: attachments.length ? attachments : undefined,
    },
  ];
  return {
    messages,
    update: {
      messages: [...messages, { id: uid(), role: "assistant", text: "" }],
      busy: true,
      error: "",
      note: "",
      resolvedModel: "",
      updatedAt: now,
      title: DEFAULT_TITLES.has(panel.title) ? titleFrom(text) : panel.title,
    },
  };
}

/** Folds one streamed chat event into a panel. An answer that ends empty is
 * dropped; one that ends with text is stamped with the model that produced it,
 * so switching models later still shows what answered each earlier turn. */
export function applyChatEvent(
  panel: Panel,
  event: ChatEvent,
  now = Date.now(),
): Panel {
  const messages = [...(panel.messages || [])];
  const last = messages.at(-1);
  if (event.text && last?.role === "assistant")
    messages[messages.length - 1] = { ...last, text: last.text + event.text };
  if (event.done && last?.role === "assistant" && !messages.at(-1)?.text)
    messages.pop();
  const final = messages.at(-1);
  if (event.done && final?.role === "assistant" && final.text)
    messages[messages.length - 1] = {
      ...final,
      model: event.model || panel.resolvedModel,
    };
  return {
    ...panel,
    messages,
    busy: !event.done,
    error: event.error || panel.error,
    note: event.done ? undefined : (event.note ?? panel.note),
    resolvedModel: event.model || panel.resolvedModel,
    usage: event.usage || panel.usage,
    updatedAt: event.done ? now : panel.updatedAt,
  };
}
