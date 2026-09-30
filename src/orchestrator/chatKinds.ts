// Chat session kinds: the Chat view and Brainstorm each keep their own
// sessions on the repo's one orchd chat store. orchd owns the kinds: every
// `chat.*` call names one, each kind has its own current session, and a
// brainstorm session runs the brainstorm prompt and keeps a task draft.
import { orchestratorClient, type OrchestratorClient } from "./client.ts";
import type { ChatKind, ChatSessionList, ChatThread } from "./types.ts";

export type { ChatKind };
export type KindedList = ChatSessionList;

export function listSessions(
  repo: string,
  kind: ChatKind,
  client: OrchestratorClient = orchestratorClient,
) {
  return client.chatList(repo, kind);
}

/** The kind's current session (orchd starts an empty one when the kind has
 * none yet) and its session list. */
export async function enterKind(
  repo: string,
  kind: ChatKind,
  client: OrchestratorClient = orchestratorClient,
): Promise<{ thread: ChatThread; list: KindedList }> {
  const [thread, list] = await Promise.all([
    client.chatGet(repo, kind),
    client.chatList(repo, kind),
  ]);
  return { thread, list };
}

/** A chat event's kind; an older daemon names none, and chat was its only
 * kind. */
export function eventKind(event: { kind?: ChatKind }): ChatKind {
  return event.kind ?? "chat";
}

/** Whether `id` is a listed session of `kind`. A session the list does not
 * know yet counts as a chat, the default kind. */
export function isOfKind(
  list: KindedList | null,
  id: string,
  kind: ChatKind,
): boolean {
  const session = list?.sessions.find((s) => s.id === id);
  return (session?.kind ?? "chat") === kind;
}
