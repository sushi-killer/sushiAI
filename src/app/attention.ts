import { isHidden } from "./workspaceMerge.ts";
import type {
  AttentionNotice,
  ConnectionProfile,
  Panel,
  Workspace,
} from "../types";

/** Main validates a notice against its own caps and throws past them, while
 * the renderer sends fire-and-forget: a pane whose title carries a long command
 * line would lose its notification and every reminder behind it. Cut to fit
 * instead - the caps mirror `validateNotice` in electron/attention.cjs. */
export function fitNotice(notice: AttentionNotice): AttentionNotice {
  const cut = (text: string, max: number) =>
    text.length <= max ? text : `${text.slice(0, max - 1)}\u2026`;
  return {
    ...notice,
    title: cut(notice.title, 120),
    body: cut(notice.body, 300),
  };
}

/** Minutes after a panel enters `blocked` that a reminder fires, each once. */
export const REMINDER_MINUTES = [5, 10, 20] as const;

type TrackedPanel = {
  /** The agent panel's own status the last time it was observed. */
  status: string;
  /** When it entered that status, ms epoch - the anchor both `dueReminders`
   * and the Inbox row's "time in state" measure from. */
  since: number;
  /** Reminder minutes already fired for the current `blocked` episode; reset
   * whenever the status changes, so leaving and re-entering `blocked`
   * restarts the schedule. */
  remindersFired: number[];
};

/** Everything the Inbox needs to remember across renders: per-agent-panel
 * status/timing, and which finished panels the user has not looked at yet.
 * Kept in memory only (see `useAttention`) - a restart treats everything as
 * seen, which is fine since nothing here is persisted. */
export type AttentionState = {
  panels: Record<string, TrackedPanel>;
  unseen: Set<string>;
};

export function createAttentionState(): AttentionState {
  return { panels: {}, unseen: new Set() };
}

export type AttentionEvent = {
  kind: "blocked" | "done";
  workspace: Workspace;
  panel: Panel;
};

/** A panel a reminder points at, without the workspace/panel objects - the
 * caller (`useAttention`) still has the live workspace list to resolve those,
 * and a pure timer tick has nothing else to look them up with anyway. */
export type DueReminder = { panelId: string; minutes: number };

export type InboxGroupKey = "blocked" | "done" | "working" | "idle" | "shells";

export type InboxRow = {
  workspace: Workspace;
  panel: Panel;
  group: InboxGroupKey;
  /** When this panel entered its current tracked status, ms epoch - `null`
   * when it was never tracked (a plain shell, most of the time). */
  since: number | null;
};

export type InboxGroup = {
  key: InboxGroupKey;
  label: string;
  rows: InboxRow[];
};

const GROUPS: { key: InboxGroupKey; label: string }[] = [
  { key: "blocked", label: "Needs input" },
  { key: "done", label: "Done · not seen" },
  { key: "working", label: "Working" },
  { key: "idle", label: "Idle" },
  { key: "shells", label: "Shells" },
];

/** A panel that can wait for input or finish: a dedicated agent panel, or a
 * terminal whose session runs an agent. Kept in step with `bucketFor` on
 * purpose - a panel the Inbox lists as blocked must also be one `observe`
 * times and notifies about. */
function isAgentPanel(panel: Panel): boolean {
  return panel.kind === "agent" || (panel.kind === "terminal" && !!panel.agent);
}

/** Which Inbox group a panel belongs to, or `null` when it is not listed at
 * all. Only a session belongs in an attention queue: a terminal or an agent.
 * A browser, a file tree, a chat thread or an extension surface is a view of
 * the project, nothing that can wait for you or finish. Terminal and agent
 * are handled alike, because a terminal whose session runs an agent can
 * be `working` or `blocked` just like a dedicated agent panel. */
function bucketFor(panel: Panel, unseen: Set<string>): InboxGroupKey | null {
  if (panel.kind !== "terminal" && panel.kind !== "agent") return null;
  // Only an agent panel is observed, so only an agent panel may enter a status
  // bucket: a shell filed under "Needs input" would count toward the badge and
  // the tray, yet never be timed, notified, cleared or cleaned up.
  if (!isAgentPanel(panel)) return "shells";
  if (panel.status === "blocked") return "blocked";
  if (panel.status === "done") return unseen.has(panel.id) ? "done" : "idle";
  if (panel.status === "working") return "working";
  return "idle";
}

/** Advances the tracked state one tick. The status is the panel's own, which
 * the daemon's `agentStatus` drives (see `reconcileSessions`): `blocked` waits
 * for the owner, and `working` then idle arrives here as `done`. It records every agent panel's status,
 * silently the first time (no startup spam), and returns an event for every
 * real transition into `blocked` or `done`. A panel entering `done` becomes
 * unseen; leaving `blocked` clears its reminder schedule (bundled into the
 * same status-change reset below). Panels that disappeared - closed, or their
 * workspace closed - are dropped. */
export function observe(
  state: AttentionState,
  workspaces: Workspace[],
  now: number,
): { state: AttentionState; events: AttentionEvent[] } {
  const panels: Record<string, TrackedPanel> = { ...state.panels };
  const unseen = new Set(state.unseen);
  const events: AttentionEvent[] = [];
  const present = new Set<string>();
  for (const workspace of workspaces) {
    for (const panel of workspace.panels) {
      if (!isAgentPanel(panel)) continue;
      present.add(panel.id);
      const status = panel.status || "idle";
      const previous = panels[panel.id];
      if (!previous) {
        // First sight of this panel - at startup that is every agent on every
        // host. Recording it silently is the point: an agent that finished
        // while the app was closed is not news, so it never counts as unseen
        // here (`blocked` still shows up, because it genuinely waits).
        panels[panel.id] = { status, since: now, remindersFired: [] };
        continue;
      }
      if (previous.status === status) continue;
      if (status === "blocked")
        events.push({ kind: "blocked", workspace, panel });
      else if (status === "done") {
        events.push({ kind: "done", workspace, panel });
        unseen.add(panel.id);
      }
      panels[panel.id] = { status, since: now, remindersFired: [] };
    }
  }
  for (const id of Object.keys(panels))
    if (!present.has(id)) {
      delete panels[id];
      unseen.delete(id);
    }
  return { state: { panels, unseen }, events };
}

/** The reminders due right now - every `blocked` panel whose elapsed time has
 * crossed a `REMINDER_MINUTES` mark it has not fired yet - and the state with
 * those marks recorded so each fires only once. */
export function dueReminders(
  state: AttentionState,
  now: number,
): { reminders: DueReminder[]; state: AttentionState } {
  const panels: Record<string, TrackedPanel> = { ...state.panels };
  const reminders: DueReminder[] = [];
  for (const [panelId, tracked] of Object.entries(panels)) {
    if (tracked.status !== "blocked") continue;
    const elapsedMinutes = (now - tracked.since) / 60000;
    let fired = tracked.remindersFired;
    for (const minutes of REMINDER_MINUTES) {
      if (elapsedMinutes >= minutes && !fired.includes(minutes)) {
        reminders.push({ panelId, minutes });
        fired = [...fired, minutes];
      }
    }
    if (fired !== tracked.remindersFired)
      panels[panelId] = { ...tracked, remindersFired: fired };
  }
  return { reminders, state: { panels, unseen: state.unseen } };
}

export function markSeen(
  state: AttentionState,
  panelId: string,
): AttentionState {
  if (!state.unseen.has(panelId)) return state;
  const unseen = new Set(state.unseen);
  unseen.delete(panelId);
  return { panels: state.panels, unseen };
}

/** The Inbox's five groups, in display order, each carrying every listed
 * panel that belongs to it - a workspace on a host hidden from the sidebar
 * (see `isHidden`) never contributes a row. Groups with no rows are still
 * returned; the page hides them. */
export function inboxGroups(
  workspaces: Workspace[],
  state: AttentionState,
  profiles: ConnectionProfile[],
): InboxGroup[] {
  const rows: InboxRow[] = [];
  for (const workspace of workspaces) {
    if (isHidden(workspace.connection, profiles)) continue;
    for (const panel of workspace.panels) {
      const group = bucketFor(panel, state.unseen);
      if (!group) continue;
      rows.push({
        workspace,
        panel,
        group,
        since: state.panels[panel.id]?.since ?? null,
      });
    }
  }
  return GROUPS.map(({ key, label }) => ({
    key,
    label,
    rows: rows.filter((row) => row.group === key),
  }));
}

/** What the Dock badge and the Inbox nav count show: needing input, plus
 * finished-and-unseen. Hidden hosts are skipped exactly as `inboxGroups`
 * skips them, so the count never promises a row the Inbox cannot show.
 * `moduleItems` are the rows the modules put in the Inbox; those that wait
 * on the owner (answer, decide) add to it, a review row does not. */
export function waitingCount(
  workspaces: Workspace[],
  state: AttentionState,
  profiles: ConnectionProfile[],
  moduleItems: { kind: string }[] = [],
): number {
  let count = moduleItems.filter((item) => item.kind !== "review").length;
  for (const workspace of workspaces) {
    if (isHidden(workspace.connection, profiles)) continue;
    for (const panel of workspace.panels) {
      const group = bucketFor(panel, state.unseen);
      if (group === "blocked" || group === "done") count++;
    }
  }
  return count;
}

/** How many sessions are running right now: what turns the menu bar mark
 * from quiet to working. Skips hidden hosts like `waitingCount` does. */
export function workingCount(
  workspaces: Workspace[],
  state: AttentionState,
  profiles: ConnectionProfile[],
): number {
  let count = 0;
  for (const workspace of workspaces) {
    if (isHidden(workspace.connection, profiles)) continue;
    for (const panel of workspace.panels)
      if (bucketFor(panel, state.unseen) === "working") count++;
  }
  return count;
}
