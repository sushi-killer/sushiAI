// Chat session kinds: the Chat view and Brainstorm each keep their own
// sessions on the repo's one orchd chat store. orchd is adding kinds itself
// (`chat.new {kind}`, `chat.list/get {kind}`, `kind` on each summary, and a
// brainstorm system prompt); until it lands this module fakes them on the
// client, and it is the only file that has to change when it does. Every
// call already passes `kind`, which today's daemon ignores.
import type { ChatSessionSummary, ChatThread } from "./types";

export type ChatKind = "chat" | "brainstorm";

/** A session summary plus the optional fields orchd is adding. */
export type KindedSession = ChatSessionSummary & {
  kind?: string;
  preview?: string;
  updatedAt?: number;
};
export type KindedList = { current: string; sessions: KindedSession[] };

function call<T>(method: string, params: Record<string, unknown>): Promise<T> {
  if (!window.bridge)
    return Promise.reject(new Error("Open the desktop app first."));
  return window.bridge.orchestrator(method, params) as Promise<T>;
}

/** Brainstorm's role, sent once as a hidden tail of the session's first
 * message while orchd has no brainstorm system prompt of its own. It starts
 * on a new paragraph so the session title (orchd takes the first message's
 * first 60 characters) keeps only what the owner wrote. */
export const BRAINSTORM_MARKER = "[sushiAI brainstorm mode]";
export const BRAINSTORM_PREAMBLE =
  `\n\n${BRAINSTORM_MARKER} You are helping the owner shape one feature into a single orchd task before anything runs. ` +
  "Do not create, start or change tasks and do not change files. Ask one short question at a time about what is still open. " +
  "When the question has clear alternatives, end the reply with a fenced ```sushi-options block holding a JSON array of 2-4 short answers. " +
  "After every reply, also add one fenced ```sushi-plan block holding JSON " +
  '{"title": string, "goal": string, "criteria": string[], "dependsOn": string[], "tier": "mechanical"|"standard"|"hard", "base": string} ' +
  "with everything settled so far (omit what you do not know yet). Reply in the owner's language, briefly.";

type Stored = {
  brainstorm: string[];
  last: Partial<Record<ChatKind, string>>;
  seen: Record<string, { ts: number; preview: string }>;
};

function storeKey(repo: string): string {
  return `sushiai.orchestrator.chatKinds:${repo}`;
}

function readStore(repo: string): Stored {
  try {
    const raw = window.localStorage.getItem(storeKey(repo));
    const parsed = raw ? (JSON.parse(raw) as Partial<Stored>) : {};
    return {
      brainstorm: Array.isArray(parsed.brainstorm) ? parsed.brainstorm : [],
      last: parsed.last ?? {},
      seen: parsed.seen ?? {},
    };
  } catch {
    return { brainstorm: [], last: {}, seen: {} };
  }
}

function writeStore(repo: string, change: (stored: Stored) => void) {
  const stored = readStore(repo);
  change(stored);
  try {
    window.localStorage.setItem(storeKey(repo), JSON.stringify(stored));
  } catch {
    // Private window or blocked storage: kinds last until the app restarts.
  }
}

/** True once orchd tags its summaries with a kind. */
function daemonKinds(list: KindedList | null): boolean {
  return !!list?.sessions.some((s) => typeof s.kind === "string");
}

export function kindOf(repo: string, session: KindedSession): ChatKind {
  if (typeof session.kind === "string")
    return session.kind === "brainstorm" ? "brainstorm" : "chat";
  return readStore(repo).brainstorm.includes(session.id)
    ? "brainstorm"
    : "chat";
}

/** The sessions of one kind, oldest first as orchd keeps them. */
export function sessionsOfKind(
  repo: string,
  list: KindedList | null,
  kind: ChatKind,
): KindedSession[] {
  return (list?.sessions ?? []).filter((s) => kindOf(repo, s) === kind);
}

export function isOfKind(
  repo: string,
  list: KindedList | null,
  id: string,
  kind: ChatKind,
): boolean {
  const session = list?.sessions.find((s) => s.id === id) ?? {
    id,
    busy: false,
  };
  return kindOf(repo, session) === kind;
}

function remember(repo: string, kind: ChatKind, thread: ChatThread) {
  writeStore(repo, (stored) => {
    stored.last[kind] = thread.id;
    if (kind === "brainstorm" && !stored.brainstorm.includes(thread.id))
      stored.brainstorm.push(thread.id);
  });
  noteThread(repo, thread);
  return thread;
}

export function listSessions(repo: string, kind: ChatKind) {
  return call<KindedList>("chat.list", { repo, kind });
}

export function newSession(repo: string, kind: ChatKind) {
  return call<ChatThread>("chat.new", { repo, kind }).then((thread) =>
    remember(repo, kind, thread),
  );
}

export function switchSession(repo: string, kind: ChatKind, id: string) {
  return call<ChatThread>("chat.switch", { repo, id }).then((thread) =>
    remember(repo, kind, thread),
  );
}

/** Makes this view's last session current (a new one when it has none) and
 * returns it. Refused by orchd while another session's reply is live. */
export async function enterKind(
  repo: string,
  kind: ChatKind,
): Promise<{ thread: ChatThread; list: KindedList }> {
  const list = await listSessions(repo, kind);
  const mine = sessionsOfKind(repo, list, kind);
  const current = mine.find((s) => s.id === list.current);
  const last = readStore(repo).last[kind];
  const target =
    current ?? mine.find((s) => s.id === last) ?? mine[mine.length - 1];
  const thread = !target
    ? await newSession(repo, kind)
    : target.id === list.current
      ? remember(repo, kind, await call<ChatThread>("chat.get", { repo, kind }))
      : await switchSession(repo, kind, target.id);
  return { thread, list: await listSessions(repo, kind) };
}

/** Sends on the current session; a brainstorm's first message carries the
 * brainstorm role until orchd applies it itself. */
export function sendInKind(
  repo: string,
  kind: ChatKind,
  text: string,
  thread: ChatThread | null,
  list: KindedList | null,
) {
  const preamble =
    kind === "brainstorm" && !thread?.messages.length && !daemonKinds(list);
  return call<Record<string, never>>("chat.send", {
    repo,
    text: preamble ? `${text}${BRAINSTORM_PREAMBLE}` : text,
  });
}

/** A message as the owner wrote it, without the hidden brainstorm role. */
export function visibleText(text: string): string {
  const at = text.indexOf(`\n\n${BRAINSTORM_MARKER}`);
  return at < 0 ? text : text.slice(0, at);
}

/** A session title as the list shows it: its first line only, so a
 * brainstorm role cut into the title never shows. */
export function visibleTitle(title: string | undefined): string {
  return (title ?? "").split("\n")[0].trim();
}

/** Keeps the last message of every thread this app has seen, for the
 * session list's preview and time while orchd's summaries carry neither. */
export function noteThread(repo: string, thread: ChatThread) {
  const last = thread.messages[thread.messages.length - 1];
  const seen = readStore(repo).seen[thread.id];
  if (!last) {
    if (seen)
      writeStore(repo, (stored) => {
        delete stored.seen[thread.id];
      });
    return;
  }
  const preview = visibleText(last.text).replace(/\s+/g, " ").trim();
  if (seen && seen.ts === last.ts && seen.preview === preview) return;
  writeStore(repo, (stored) => {
    stored.seen[thread.id] = { ts: last.ts, preview: preview.slice(0, 160) };
  });
}

/** A session's time and one-line preview: orchd's own fields when present,
 * else what this app last saw of it; absent when neither knows. */
export function sessionMeta(
  repo: string,
  session: KindedSession,
): { time?: number; preview?: string } {
  const seen = readStore(repo).seen[session.id];
  return {
    time: session.updatedAt ?? seen?.ts,
    preview: session.preview ?? seen?.preview,
  };
}
