import type { DaemonAsk, DaemonEvent, DaemonSession } from "../types";

/** A permission ask an agent is waiting on, with the host whose daemon owns it. */
export type OpenAsk = DaemonAsk & { host: string };

/** Open asks by host, then by ask id. */
export type AsksByHost = Record<string, Record<string, OpenAsk>>;

export const noAsks: AsksByHost = {};

const withHost = (host: string, asks: DaemonAsk[] | undefined): OpenAsk[] =>
  (asks ?? []).map((ask) => ({ ...ask, host }));

function setHost(
  state: AsksByHost,
  host: string,
  next: Record<string, OpenAsk>,
): AsksByHost {
  const known = state[host] ?? {};
  const same =
    Object.keys(known).length === Object.keys(next).length &&
    Object.keys(next).every((id) => known[id] === next[id]);
  if (same) return state;
  return { ...state, [host]: next };
}

/** A full `session.list`: the asks it carries are every ask the host has open. */
export function applyAskList(
  state: AsksByHost,
  host: string,
  sessions: DaemonSession[],
): AsksByHost {
  const known = state[host] ?? {};
  const next: Record<string, OpenAsk> = {};
  for (const session of sessions)
    for (const ask of withHost(host, session.asks)) {
      const old = known[ask.askId];
      next[ask.askId] =
        old && old.session === ask.session && old.input === ask.input
          ? old
          : ask;
    }
  return setHost(state, host, next);
}

/** One daemon notification. A new ask is added; `session.askClosed` removes it
 * however it was settled (Allow or Deny here, an answer in the terminal, a
 * timeout); a session that is updated carries its own open asks, and one that
 * is removed or exited has none. */
export function applyAskEvent(
  state: AsksByHost,
  event: Pick<DaemonEvent, "method" | "params" | "host">,
): AsksByHost {
  const { host } = event;
  const known = state[host] ?? {};
  switch (event.method) {
    case "session.ask": {
      const params = event.params as DaemonAsk;
      if (!params?.askId) return state;
      return setHost(state, host, {
        ...known,
        [params.askId]: { ...params, host },
      });
    }
    case "session.askClosed": {
      const { askId } = event.params as { askId: string };
      if (!(askId in known)) return state;
      const rest = { ...known };
      delete rest[askId];
      return setHost(state, host, rest);
    }
    case "session.created":
    case "session.updated": {
      const session = event.params as DaemonSession;
      const next = Object.fromEntries(
        Object.entries(known).filter(([, ask]) => ask.session !== session.id),
      );
      for (const ask of withHost(host, session.asks)) {
        const old = known[ask.askId];
        next[ask.askId] = old && old.input === ask.input ? old : ask;
      }
      return setHost(state, host, next);
    }
    case "session.removed":
    case "session.exited": {
      const { id } = event.params as { id: string };
      return setHost(
        state,
        host,
        Object.fromEntries(
          Object.entries(known).filter(([, ask]) => ask.session !== id),
        ),
      );
    }
    default:
      return state;
  }
}

/** The open asks of one session, oldest first. */
export function asksOf(
  state: AsksByHost,
  host: string,
  sessionId: string,
): OpenAsk[] {
  return Object.values(state[host] ?? {}).filter(
    (ask) => ask.session === sessionId,
  );
}

const SUMMARY_KEYS = [
  "command",
  "file_path",
  "path",
  "url",
  "pattern",
  "query",
  "description",
];
const SUMMARY_MAX = 140;

const clip = (text: string) => {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length <= SUMMARY_MAX
    ? line
    : `${line.slice(0, SUMMARY_MAX - 1)}…`;
};

/** A short, one-line description of what the tool will do, from its input. */
export function summarizeAskInput(input: unknown): string {
  if (typeof input === "string") return clip(input);
  if (input && typeof input === "object") {
    const fields = input as Record<string, unknown>;
    for (const key of SUMMARY_KEYS)
      if (typeof fields[key] === "string" && fields[key])
        return clip(fields[key] as string);
    try {
      const text = JSON.stringify(input);
      return text === "{}" ? "" : clip(text);
    } catch {
      return "";
    }
  }
  return input == null ? "" : clip(String(input));
}

/** The request `askRespond` takes for one button press. */
export function askDecision(
  ask: OpenAsk,
  decision: "allow" | "deny",
): [string, { sessionId: string; askId: string; decision: "allow" | "deny" }] {
  return [ask.host, { sessionId: ask.session, askId: ask.askId, decision }];
}
