import type {
  Bridge,
  DaemonAsk,
  DaemonEvent,
  DaemonSession,
  DaemonState,
  Panel,
  Workspace,
} from "./types";

/** What the desktop knows about one host's sessions. */
export type HostSessions = {
  /** The host's connection is up. Nothing ends while this is false. */
  ready: boolean;
  /** A full `session.list` was applied for the current connection. Absence
   * from `sessions` means "gone" only after this is true. */
  listed: boolean;
  sessions: Record<string, DaemonSession>;
};
export type SessionsByHost = Record<string, HostSessions>;

type StatusParams = {
  id: string;
  status?: DaemonSession["agentStatus"];
  statusSource?: DaemonSession["statusSource"];
  statusSince?: number;
};
type MetaParams = {
  id: string;
  agentSession?: string | null;
  transcriptPath?: string | null;
  /** Absent from an older daemon: the field is left as it was. */
  foregroundAgent?: string | null;
  foregroundCwd?: string | null;
};

const withoutAsks = (session: DaemonSession): DaemonSession => {
  const next = { ...session };
  delete next.asks;
  return next;
};

export const emptyHost = (): HostSessions => ({
  ready: false,
  listed: false,
  sessions: {},
});

/** The endpoint, and the daemon host, of This Mac. */
export const LOCAL_ENDPOINT = "local";

/** The daemon host of a workspace connection: "local" for this Mac (any
 * non-ssh endpoint), else the connection id. */
export function daemonHost(connection: string | undefined): string {
  return connection?.startsWith("ssh:") ? connection.slice(4) : LOCAL_ENDPOINT;
}

/** The stable panel id of a daemon session on an endpoint. */
export function sessionPanelId(endpoint: string, sessionId: string): string {
  return `session:${encodeURIComponent(endpoint)}:${encodeURIComponent(sessionId)}`;
}

/** The full list of a host replaces what was known; events that arrived while
 * the list was in flight are replayed on top by the caller. */
export function applySessionList(
  host: HostSessions,
  sessions: DaemonSession[],
): HostSessions {
  return {
    ...host,
    listed: true,
    sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
  };
}

/** One notification applied to a host's sessions. Returns the same object when
 * the event changes nothing. `session.resync` and `session.open` are the
 * caller's business, not session state. */
export function applyDaemonEvent(
  host: HostSessions,
  event: Pick<DaemonEvent, "method" | "params">,
): HostSessions {
  const { sessions } = host;
  switch (event.method) {
    case "session.created":
    case "session.updated": {
      const session = event.params as DaemonSession;
      return { ...host, sessions: { ...sessions, [session.id]: session } };
    }
    case "session.removed": {
      const { id } = event.params as { id: string };
      if (!(id in sessions)) return host;
      const rest = { ...sessions };
      delete rest[id];
      return { ...host, sessions: rest };
    }
    case "session.ask": {
      // The session record carries its open asks, so Inbox reads them here.
      const ask = event.params as DaemonAsk;
      const known = ask?.askId ? sessions[ask.session] : undefined;
      if (!known) return host;
      const rest = (known.asks ?? []).filter((a) => a.askId !== ask.askId);
      return {
        ...host,
        sessions: {
          ...sessions,
          [known.id]: { ...known, asks: [...rest, ask] },
        },
      };
    }
    case "session.askClosed": {
      // Settled however: Allow or Deny here, an answer in the terminal, a timeout.
      const { askId } = event.params as { askId: string };
      const known = Object.values(sessions).find((s) =>
        s.asks?.some((a) => a.askId === askId),
      );
      if (!known) return host;
      const asks = known.asks!.filter((a) => a.askId !== askId);
      const next = asks.length ? { ...known, asks } : withoutAsks(known);
      return { ...host, sessions: { ...sessions, [known.id]: next } };
    }
    case "session.exited": {
      const { id, code } = event.params as { id: string; code: number | null };
      const known = sessions[id];
      if (!known) return host;
      // An exited session has no open asks.
      const rest = withoutAsks(known);
      return {
        ...host,
        sessions: {
          ...sessions,
          [id]: {
            ...rest,
            status: "exited",
            agentStatus: known.agentStatus && "exited",
            ...(code === null ? {} : { exitCode: code }),
          },
        },
      };
    }
    case "session.status": {
      const p = event.params as StatusParams;
      const known = sessions[p.id];
      if (!known) return host;
      return {
        ...host,
        sessions: {
          ...sessions,
          [p.id]: {
            ...known,
            agentStatus: p.status,
            statusSource: p.statusSource,
            statusSince: p.statusSince,
          },
        },
      };
    }
    case "session.meta": {
      const p = event.params as MetaParams;
      const known = sessions[p.id];
      if (!known) return host;
      return {
        ...host,
        sessions: {
          ...sessions,
          [p.id]: {
            ...known,
            agentSession: p.agentSession ?? undefined,
            transcriptPath: p.transcriptPath ?? undefined,
            ...("foregroundAgent" in p
              ? {
                  foregroundAgent: p.foregroundAgent ?? undefined,
                  foregroundCwd: p.foregroundCwd ?? undefined,
                }
              : {}),
          },
        },
      };
    }
    default:
      return host;
  }
}

/** The event stream of one host: the newest connection generation seen, the
 * id of the latest full list, and the events that arrived while that list was
 * in flight (the list is a snapshot from before them, so they are replayed on
 * top of it). Immutable: every step returns the next feed. */
export type HostFeed = {
  generation: number;
  token: number;
  pending: DaemonEvent[] | null;
};
export const emptyFeed = (): HostFeed => ({
  generation: -1,
  token: 0,
  pending: null,
});

/** One event for a host. An older generation is dropped. `session.resync`
 * asks the caller to list again. Anything else applies now and, while a list
 * is in flight, is also kept for replay. */
export function feedEvent(
  feed: HostFeed,
  host: HostSessions,
  event: DaemonEvent,
): { feed: HostFeed; host: HostSessions; relist: boolean } {
  if (event.generation < feed.generation) return { feed, host, relist: false };
  if (event.method === "session.resync") return { feed, host, relist: true };
  return {
    feed: feed.pending ? { ...feed, pending: [...feed.pending, event] } : feed,
    host: applyDaemonEvent(host, event),
    relist: false,
  };
}

/** A full list starts: later events are kept until it lands. */
export function startList(feed: HostFeed): { feed: HostFeed; token: number } {
  const token = feed.token + 1;
  return { feed: { ...feed, token, pending: [] }, token };
}

/** A full list landed. A list that a newer one replaced is dropped (undefined);
 * otherwise it replaces what was known and the kept events replay on top. */
export function finishList(
  feed: HostFeed,
  token: number,
  host: HostSessions,
  sessions: DaemonSession[],
): { feed: HostFeed; host: HostSessions } | undefined {
  if (feed.token !== token) return undefined;
  let next = applySessionList(host, sessions);
  for (const event of feed.pending ?? []) next = applyDaemonEvent(next, event);
  return { feed: { ...feed, pending: null }, host: next };
}

/** A full list failed: stop keeping events, unless a newer list took over. */
export function failList(feed: HostFeed, token: number): HostFeed {
  return feed.token === token ? { ...feed, pending: null } : feed;
}

/** The panel status the UI draws. A finished turn reads "done" until the
 * attention layer has seen it, then "idle". "starting" is neutral (no green
 * dot, not counted as working): an agent may wait at its prompt with no hook
 * yet, and Codex fires its first hook only with the first prompt. Terminals
 * have no status. */
function panelStatus(
  session: DaemonSession,
  previous: string | undefined,
): string | undefined {
  // A sleeping or waking agent is alive, not ended; it counts like idle.
  if (session.status === "hibernated") return "sleeping";
  if (session.status === "waking") return "waking";
  switch (session.agentStatus) {
    case "starting":
      return "starting";
    case "working":
      return "working";
    case "blocked":
      return "blocked";
    case "idle":
      return previous === "working" ||
        previous === "blocked" ||
        previous === "done"
        ? "done"
        : "idle";
    default:
      return undefined;
  }
}

/** A session the host no longer runs. The panel keeps its slot and its saved
 * state; it shows "Session ended" with Reopen until it is reopened or closed. */
function endPanel(panel: Panel, session?: DaemonSession): Panel {
  const terminal = panel.kind === "terminal";
  const next: Panel = { ...panel, ended: true };
  delete next.status;
  if (session) {
    // The host still has the record, so it is what Reopen continues: a shell
    // that was left after its agent quit has nothing to continue. Only a host
    // that forgot the session leaves the panel's own copy.
    const agent =
      session.agent ??
      session.foregroundAgent ??
      (terminal ? undefined : panel.agent);
    const conversation =
      session.agentSession || (terminal ? undefined : panel.agentSession);
    setOrDelete(next, "agent", agent);
    setOrDelete(next, "agentSession", conversation);
    setOrDelete(next, "agentCwd", terminal ? session.foregroundCwd : undefined);
  }
  return unchanged(panel, next);
}

function setOrDelete<K extends "agent" | "agentSession" | "agentCwd">(
  panel: Panel,
  key: K,
  value: string | undefined,
) {
  if (value) panel[key] = value;
  else delete panel[key];
}

/** The old panel when `next` differs from it in no field. */
function unchanged(panel: Panel, next: Panel): Panel {
  const keys = new Set([...Object.keys(panel), ...Object.keys(next)]);
  for (const key of keys)
    if (
      (panel as Record<string, unknown>)[key] !==
      (next as Record<string, unknown>)[key]
    )
      return next;
  return panel;
}

function livePanel(panel: Panel, session: DaemonSession): Panel {
  const next: Panel = { ...panel };
  delete next.ended;
  const status = panelStatus(session, panel.status);
  if (status === undefined) delete next.status;
  else next.status = status;
  const title = session.title?.trim();
  if (title) next.title = title;
  if (session.cwd) next.paneCwd = session.cwd;
  if (session.agentSession) next.agentSession = session.agentSession;
  if (session.pinned) next.keepAwake = true;
  else delete next.keepAwake;
  // The icon follows the agent the daemon runs in the session. A terminal also
  // follows one the owner started by hand, and drops it (with its conversation
  // id and folder) when that agent is gone.
  const agent =
    session.agent ??
    (panel.kind === "terminal" ? session.foregroundAgent : undefined);
  if (agent) next.agent = agent;
  if (panel.kind === "terminal") {
    if (!agent) delete next.agent;
    if (!session.agentSession) delete next.agentSession;
    setOrDelete(next, "agentCwd", session.foregroundCwd);
  }
  return unchanged(panel, next);
}

/** Brings the panels bound to a daemon session in line with what each host
 * lists. Known panels take the session's status, title and folder; a session
 * that exited or is gone ends its panel; `detached` is still live. A host that
 * is not ready, or whose full list has not arrived, changes nothing. A session
 * the desktop did not create is never adopted: panels are only updated. Returns
 * the same array when nothing changes. */
export function reconcileSessions(
  workspaces: Workspace[],
  hosts: SessionsByHost,
): Workspace[] {
  let changed = false;
  const next = workspaces.map((workspace) => {
    const host = hosts[daemonHost(workspace.connection)];
    if (!host?.ready || !host.listed) return workspace;
    let touched = false;
    const panels = workspace.panels.map((panel) => {
      if (!panel.sessionId) return panel;
      const session = host.sessions[panel.sessionId];
      const updated =
        !session || session.status === "exited"
          ? endPanel(panel, session)
          : livePanel(panel, session);
      if (updated !== panel) touched = true;
      return updated;
    });
    if (!touched) return workspace;
    changed = true;
    return { ...workspace, panels };
  });
  return changed ? next : workspaces;
}

/** A permission ask an agent is waiting on, with the host whose daemon owns it. */
export type OpenAsk = DaemonAsk & { host: string };

/** The open asks of one session, oldest first. */
export function asksOf(
  hosts: SessionsByHost,
  host: string,
  sessionId: string,
): OpenAsk[] {
  return (hosts[host]?.sessions[sessionId]?.asks ?? []).map((ask) => ({
    ...ask,
    host,
  }));
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

/** True when the host's daemon reported the capability in its hello. */
export function hostSupports(
  states: Pick<DaemonState, "host" | "state" | "capabilities">[],
  host: string,
  capability: string,
): boolean {
  return states.some(
    (state) =>
      state.host === host &&
      state.state === "ready" &&
      !!state.capabilities?.includes(capability),
  );
}

/** The idle-sleep choices of Settings -> General, in seconds; 0 is off. */
export const HIBERNATE_CHOICES = [
  { secs: 0, label: "Off" },
  { secs: 3600, label: "1 h" },
  { secs: 14400, label: "4 h" },
  { secs: 43200, label: "12 h" },
] as const;
export const DEFAULT_HIBERNATE_SECS = 14400;

/** Pushes the idle-sleep delay to one host, only when it can hibernate. */
export async function configureHibernation(
  bridge: Pick<Bridge, "daemonConfigure">,
  state: Pick<DaemonState, "host" | "state" | "capabilities">,
  secs: number,
): Promise<void> {
  if (!hostSupports([state], state.host, "hibernate")) return;
  await bridge.daemonConfigure(state.host, secs);
}

/** Pushes the delay to every ready host that can hibernate. */
export async function configureAllHosts(
  bridge: Pick<Bridge, "daemonStates" | "daemonConfigure">,
  secs: number,
): Promise<void> {
  const states = await bridge.daemonStates();
  await Promise.all(
    states.map((state) =>
      configureHibernation(bridge, state, secs).catch(() => {}),
    ),
  );
}

/** `session.wake` errors that mean "start a new session instead": the host has
 * no such record (1003) or no stored launch for it (1012). */
const WAKE_FALLBACK_CODES = [1003, 1012];

/** The wake call for a Reopen, or null when Reopen must create a session: only
 * an agent panel whose session holds the agent's own conversation id wakes. A
 * terminal (shell or hand-started agent) reopens as before. */
export function wakeRequest(
  owner: Workspace,
  ended: Panel,
  defaultEndpoint: string,
): { host: string; id: string } | null {
  if (ended.kind !== "agent" || !ended.sessionId || !ended.agentSession)
    return null;
  return {
    host: daemonHost(owner.connection || defaultEndpoint),
    id: ended.sessionId,
  };
}

/** Tries to wake the session in place. True when it is waking; false when the
 * caller must fall back to `session.create {resume}`. Any other refusal throws. */
export async function wakeInPlace(
  bridge: Pick<Bridge, "daemonStates" | "sessionWake">,
  request: { host: string; id: string },
): Promise<boolean> {
  const states = await bridge.daemonStates();
  if (!hostSupports(states, request.host, "hibernate")) return false;
  const result = await bridge.sessionWake(request.host, request.id);
  if (result.ok) return true;
  if (result.code !== undefined && WAKE_FALLBACK_CODES.includes(result.code))
    return false;
  throw new Error(result.message);
}

/** Wakes a sleeping session for a click or a keystroke. When the host has no such
 * record or no stored launch (1003, 1012) the same fallback as Reopen runs
 * (`reopen`: a new session that resumes). Returns the refusal text, or null. */
export async function wakeOrReopen(
  bridge: Pick<Bridge, "sessionWake">,
  request: { host: string; id: string },
  reopen: () => void,
): Promise<string | null> {
  const result = await bridge.sessionWake(request.host, request.id);
  if (result.ok) return null;
  if (result.code !== undefined && WAKE_FALLBACK_CODES.includes(result.code)) {
    reopen();
    return null;
  }
  return result.message;
}

/** What a terminal does with a daemon state of its host: null when it changes nothing
 * (another host, not ready, a generation already handled), else the capability read
 * again. A new generation (hello, daemon restart) also means the host forgot what it
 * was told, so the caller resets its `reported` flag and says "focused" again. */
export function hostGenerationChange(
  seen: number,
  state: Pick<DaemonState, "host" | "state" | "capabilities" | "generation">,
  host: string,
): { generation: number; canFocus: boolean } | null {
  if (state.host !== host || state.state !== "ready") return null;
  if (state.generation === seen) return null;
  return {
    generation: state.generation,
    canFocus: hostSupports([state], host, "hibernate"),
  };
}
