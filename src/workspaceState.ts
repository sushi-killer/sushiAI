import type { Layout, Panel, Workspace } from "./types";
import { contains, isValidLayout, leaf, split, uid } from "./layout.ts";
import { validRoute, type RouteRef } from "./extensions/routes.ts";
import type { ProjectGit } from "./app/useProjectGit.ts";

/** Where the snapshot lives when there is no desktop bridge (dev:web). With
 * the bridge it is <userData>/sushiai.db, written by the main process. */
export const BROWSER_KEY = "sushiai.workspace-state";
// The keys the app used before the snapshot had one owner. They are named
// here and nowhere else: `importLegacy` folds them into one snapshot once.
const LEGACY_SNAPSHOT = "sushiai.v1";
const LEGACY_MERGED = "sushiai.mergedLayouts.v1";
const LEGACY_AGENT_TABS = "sushiai.agent-tabs.v1";
const LEGACY_AGENT_FOCUS = "sushiai.agent-focus.v1";
const LEGACY_CHAT_FOCUS = "sushiai.chat-focus.v1";
export const MAX_AGENT_TABS = 30;

/** The id of a closed project: its host endpoint and folder. */
export function closedProjectKey(
  endpoint: string | undefined,
  cwd: string,
): string {
  return `closed:${endpoint || "local"}:${cwd}`;
}

export type Routine = { id: string; name: string; command: string };

/** A workspace the sidebar dropped ("Close workspace and sessions") but the
 * Dashboard still offers back - id, name, cwd, host endpoint and the git
 * identity read at close time, so it can be reopened where it left off and
 * merged with any open member of the same project (see `app/projects.ts`). */
export type ClosedProject = {
  id: string;
  name: string;
  cwd: string;
  endpoint?: string;
  /** The workspace ran daemon sessions, so reopening starts one. */
  backed: boolean;
  closedAt: number;
  git: ProjectGit;
};

/** How one project is looked at: tabs or split panels, and which pane is
 * maximized. Kept per project (see `workspace/projectView.ts`). */
export type ProjectView = { tabMode: boolean; zoomed: string | null };
export type ProjectViews = Record<string, ProjectView>;

/** An Agent-mode conversation tab. */
export type AgentTab = {
  providerId: string;
  agentId: string;
  conversationId: string;
  title: string;
};
/** Which Agent-mode provider, agent and tab (by identity) was open. */
export type AgentFocus = {
  providerId: string;
  agentId: string;
  active: string;
};

export type Saved = {
  workspaces: Workspace[];
  activeId: string;
  socket: string;
  routines: Routine[];
  fontScale: number;
  mode?: "Agent" | "Code" | "Chat";
  tabMode?: boolean;
  section?: string;
  selected?: string;
  zoomed?: string | null;
  sidebar?: boolean;
  route?: RouteRef;
  /** How the sidebar shows remote workspaces: sectioned by host, or one flat
   * list with a small tag marking which ones are remote. */
  workspaceGrouping?: "grouped" | "flat";
  closedProjects?: ClosedProject[];
  views?: ProjectViews;
  /** The combined canvas of each merged project, keyed by merge group id. */
  mergedLayouts?: Record<string, Layout>;
  agentTabs?: AgentTab[];
  agentFocus?: AgentFocus;
  /** The Chat-mode thread that was open. */
  chatFocus?: string;
};

export type LegacyStorage = {
  getItem(key: string): string | null;
  removeItem(key: string): void;
};

/** The one storage adapter behind `restore` and `saveWorkspaceState`. `read`
 * is synchronous because the first render needs the state; `write` may be
 * asynchronous, `flush` never is (it runs while the window is closing). */
export type SnapshotStore = {
  read(): string | null;
  write(text: string): void | Promise<void>;
  flush(text: string): void;
  /** Where the pre-snapshot keys live, when that place exists. */
  legacy?: LegacyStorage;
};

export function snapshotStore(): SnapshotStore {
  const legacy = typeof localStorage === "undefined" ? undefined : localStorage;
  const bridge = typeof window === "undefined" ? undefined : window.bridge;
  if (bridge)
    return {
      read: () => bridge.workspaceStateRead(),
      write: (text) => bridge.workspaceStateWrite(text),
      flush: (text) => bridge.workspaceStateFlush(text),
      legacy,
    };
  const put = (text: string) => localStorage.setItem(BROWSER_KEY, text);
  return {
    read: () => localStorage.getItem(BROWSER_KEY),
    write: put,
    flush: put,
    legacy,
  };
}

function normalizeGit(value: unknown): ProjectGit {
  const g = (value as Partial<ProjectGit>) || {};
  return {
    projectId: typeof g.projectId === "string" ? g.projectId : "",
    remote: typeof g.remote === "string" ? g.remote : "",
    commonDir: typeof g.commonDir === "string" ? g.commonDir : "",
    checkout: typeof g.checkout === "string" ? g.checkout : "",
    linkedWorktree: g.linkedWorktree === true,
    subdir: typeof g.subdir === "string" ? g.subdir : "",
    branch: typeof g.branch === "string" ? g.branch : "",
  };
}
/** Accepts only a well-formed array - anything else (missing, not an array,
 * or an entry missing the id/cwd a merge or a reopen needs) drops silently
 * rather than carrying a half-broken closed project forward. */
function normalizeClosedProjects(value: unknown): ClosedProject[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(
      (p): p is Record<string, unknown> =>
        Boolean(p) &&
        typeof p === "object" &&
        typeof (p as Record<string, unknown>).id === "string" &&
        typeof (p as Record<string, unknown>).cwd === "string",
    )
    .map((p) => ({
      id: p.id as string,
      name: typeof p.name === "string" ? p.name : "",
      cwd: p.cwd as string,
      endpoint: typeof p.endpoint === "string" ? p.endpoint : undefined,
      backed: p.backed === true,
      closedAt: typeof p.closedAt === "number" ? p.closedAt : 0,
      git: normalizeGit(p.git),
    }));
}

function normalizeViews(value: unknown): ProjectViews {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).flatMap(([key, entry]) => {
      if (!entry || typeof entry !== "object") return [];
      const view = entry as Partial<ProjectView>;
      return [
        [
          key,
          {
            tabMode: view.tabMode === true,
            zoomed: typeof view.zoomed === "string" ? view.zoomed : null,
          },
        ],
      ];
    }),
  );
}

function normalizeMergedLayouts(value: unknown): Record<string, Layout> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).filter(([, layout]) =>
      isValidLayout(layout),
    ),
  ) as Record<string, Layout>;
}

function normalizeAgentTabs(value: unknown): AgentTab[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(
      (t): t is AgentTab =>
        Boolean(t) &&
        ["providerId", "agentId", "conversationId", "title"].every(
          (key) => typeof (t as Record<string, unknown>)[key] === "string",
        ),
    )
    .map((t) => ({
      providerId: t.providerId,
      agentId: t.agentId,
      conversationId: t.conversationId,
      title: t.title,
    }))
    .slice(-MAX_AGENT_TABS);
}

function normalizeAgentFocus(value: unknown): AgentFocus {
  const row = (value || {}) as Record<string, unknown>;
  return ["providerId", "agentId", "active"].every(
    (key) => typeof row[key] === "string",
  )
    ? {
        providerId: row.providerId as string,
        agentId: row.agentId as string,
        active: row.active as string,
      }
    : { providerId: "", agentId: "", active: "" };
}

const LEGACY_KEYS = [
  LEGACY_SNAPSHOT,
  LEGACY_MERGED,
  LEGACY_AGENT_TABS,
  LEGACY_AGENT_FOCUS,
  LEGACY_CHAT_FOCUS,
];

/** The one place the pre-snapshot localStorage keys are read. Runs when no
 * snapshot exists: folds them into one, writes it, then deletes them. A
 * failed write keeps the keys, so the next start tries again. */
function importLegacy(store: SnapshotStore): string | null {
  const legacy = store.legacy;
  if (!legacy) return null;
  const json = (key: string): unknown => {
    try {
      return JSON.parse(legacy.getItem(key) || "null");
    } catch {
      return null;
    }
  };
  const drop = () => {
    for (const key of LEGACY_KEYS) legacy.removeItem(key);
  };
  const main = json(LEGACY_SNAPSHOT) as Record<string, unknown> | null;
  if (!main || typeof main !== "object" || !Array.isArray(main.workspaces)) {
    // Nothing to fold: the other keys are orphans, clear them once.
    drop();
    return null;
  }
  const text = JSON.stringify({
    ...main,
    mergedLayouts: json(LEGACY_MERGED) ?? undefined,
    agentTabs: json(LEGACY_AGENT_TABS) ?? undefined,
    agentFocus: json(LEGACY_AGENT_FOCUS) ?? undefined,
    chatFocus: legacy.getItem(LEGACY_CHAT_FOCUS) ?? undefined,
  });
  try {
    store.flush(text);
    drop();
  } catch {
    /* the import still restores this session; the keys stay for next time */
  }
  return text;
}

/** A workspace whose panels are all ended daemon sessions moves to Recently
 * closed on start. Panels that come back ended without a session (saved by an
 * older release) stay in place with Reopen, so they never count here. An
 * intentionally empty project stays in the workspace list. */
export function sweepLeftovers(saved: Saved): Saved {
  const leftover = (w: Workspace) =>
    w.panels.length > 0 && w.panels.every((p) => p.sessionId && p.ended);
  const rest = saved.workspaces.filter((w) => !leftover(w));
  if (rest.length === saved.workspaces.length || !rest.length) return saved;
  const closedAt = Date.now();
  const swept = saved.workspaces
    .filter(leftover)
    .filter((w) => w.cwd)
    .map((w) => ({
      id: closedProjectKey(w.connection, w.cwd),
      name: w.name,
      cwd: w.cwd,
      endpoint: w.connection,
      backed: true,
      closedAt,
      git: {
        projectId: "",
        remote: "",
        commonDir: "",
        checkout: "",
        linkedWorktree: false,
        subdir: "",
        branch: "",
      },
    }));
  const ids = new Set(swept.map((p) => p.id));
  const gone = new Set(
    saved.workspaces
      .filter(leftover)
      .flatMap((w) => [w.id, ...w.panels.map((p) => p.id)]),
  );
  const keep = (id: string | null | undefined) =>
    id && gone.has(id) ? undefined : id;
  return {
    ...saved,
    activeId: keep(saved.activeId) || rest[0].id,
    selected: keep(saved.selected) || "",
    zoomed: keep(saved.zoomed) || null,
    workspaces: rest,
    closedProjects: [
      ...swept.filter((p, i) => swept.findIndex((q) => q.id === p.id) === i),
      ...(saved.closedProjects || []).filter((p) => !ids.has(p.id)),
    ],
  };
}

/** A terminal or agent panel that ran without a daemon session (the host had
 * reported its state, or the agent was started) has nothing to bind to: it
 * comes back ended, with Reopen, and keeps its agent so Reopen starts the same
 * one. A panel that never ran keeps its Launch button. */
function restorePanel(panel: Panel): Panel {
  const ran =
    !panel.sessionId &&
    (panel.kind === "terminal" || panel.kind === "agent") &&
    (panel.started === true ||
      panel.status !== undefined ||
      panel.paneCwd !== undefined);
  return {
    ...panel,
    busy: false,
    started: false,
    ...(ran ? { ended: true } : {}),
    // A reply that never arrived leaves an empty bubble; drop it.
    ...(panel.messages && {
      messages: panel.messages.filter((m) => m.role === "user" || m.text),
    }),
  } as Panel;
}

const restoreWorkspace =
  (socket: string) =>
  (w: Workspace): Workspace => {
    const backed = w.panels.some((p) => p.sessionId || p.paneCwd);
    return {
      ...w,
      connection: w.connection || (backed ? socket : undefined),
      panels: w.panels.map(restorePanel),
    };
  };

export function restore(store: SnapshotStore = snapshotStore()): Saved | null {
  try {
    const value = JSON.parse(store.read() ?? importLegacy(store) ?? "null");
    if (!Array.isArray(value?.workspaces) || !value.workspaces.length)
      return null;
    const mode =
      value.mode === "Agent" || value.mode === "Chat" ? value.mode : "Code";
    const normalized: Saved = {
      ...value,
      mode,
      tabMode: value.tabMode === true,
      section: typeof value.section === "string" ? value.section : "",
      selected: typeof value.selected === "string" ? value.selected : "",
      zoomed: typeof value.zoomed === "string" ? value.zoomed : null,
      sidebar: typeof value.sidebar === "boolean" ? value.sidebar : undefined,
      route: validRoute(value.route) ? value.route : undefined,
      workspaceGrouping:
        value.workspaceGrouping === "flat" ? "flat" : "grouped",
      closedProjects: normalizeClosedProjects(value.closedProjects),
      views: normalizeViews(value.views),
      mergedLayouts: normalizeMergedLayouts(value.mergedLayouts),
      agentTabs: normalizeAgentTabs(value.agentTabs),
      agentFocus: normalizeAgentFocus(value.agentFocus),
      chatFocus: typeof value.chatFocus === "string" ? value.chatFocus : "",
      workspaces: value.workspaces.map(restoreWorkspace(value.socket)),
    };
    const migrated = sweepLeftovers(normalized);
    if (migrated !== normalized) {
      try {
        store.flush(JSON.stringify(migrated));
      } catch {
        /* Restore the migrated state; an unchanged disk retries next start. */
      }
    }
    return migrated;
  } catch {
    return null;
  }
}

/** Debounced write; the returned promise (desktop only) rejects when the main
 * process could not write the file. */
export function saveWorkspaceState(
  value: Saved,
  store: SnapshotStore = snapshotStore(),
): void | Promise<void> {
  return store.write(JSON.stringify(value));
}

/** Synchronous write for beforeunload/pagehide, where an async one would be
 * dropped with the window. */
export function flushWorkspaceState(
  value: Saved,
  store: SnapshotStore = snapshotStore(),
): void {
  store.flush(JSON.stringify(value));
}

export function initialWorkspace(cwd = ""): Workspace {
  const panels: Panel[] = [
    { id: uid(), kind: "agent", title: "Claude Code", agent: "claude" },
    { id: uid(), kind: "terminal", title: "zsh" },
    { id: uid(), kind: "browser", title: "Browser" },
    { id: uid(), kind: "chat", title: "Thread", agent: "claude", messages: [] },
  ];
  return {
    id: uid(),
    name: "sushiai",
    cwd,
    panels,
    layout: split(
      leaf(panels[0].id),
      split(
        leaf(panels[1].id),
        split(leaf(panels[2].id), leaf(panels[3].id), "column", 0.49),
        "column",
        0.28,
      ),
      "row",
      0.53,
    ),
  };
}

export const codePanels = (w: Workspace) =>
  w.panels.filter((p) => p.kind !== "chat" || contains(w.layout, p.id));
