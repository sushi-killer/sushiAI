import {
  contains,
  insert,
  leaf,
  leafIds,
  remove,
  replaceLeaf,
  split,
  swap,
  tidy,
} from "../layout.ts";
import { agentTitle } from "../app/agent-title.ts";
import { memberLabel } from "../app/workspaceMerge.ts";
import type { MergeGroup } from "../app/workspaceMerge.ts";
import { codePanels } from "../workspaceState.ts";
import { daemonHost } from "../daemonSessions.ts";
import type {
  ConnectionProfile,
  Layout,
  Panel,
  SessionLaunchRequest,
  Workspace,
} from "../types";

/** A merge group's combined layout has no workspace of its own to live in -
 * `layout` is whatever the caller (mergedLayouts.ts) reconciled for this
 * render, `setLayout` writes back through its own storage. `drop`,
 * `resizeSplit` and `tidy` in useWorkspaces.ts use this instead of the
 * active workspace's own layout whenever it is set. */
export type GroupCanvasContext = {
  group: MergeGroup;
  layout: Layout | null;
  setLayout(layout: Layout | null): void;
};

/** Adds a panel beside the existing layout. `ratio` is the share the existing
 * layout keeps, so a larger number leaves the newcomer smaller. */
export function appendPanel(
  workspace: Workspace,
  panel: Panel,
  ratio = 0.65,
): Workspace {
  return {
    ...workspace,
    panels: [...workspace.panels, panel],
    layout: workspace.layout
      ? split(workspace.layout, leaf(panel.id), "row", ratio)
      : leaf(panel.id),
  };
}

/** Puts the panel that replaces an ended session panel into its layout slot
 * and panel-list position. A panel with the new id may already be listed, so
 * it is folded in rather than duplicated. */
export function reopenInSlot(
  workspace: Workspace,
  endedId: string,
  next: Panel,
  herdrId?: string,
): Workspace {
  const listed = next.id !== endedId && contains(workspace.layout, next.id);
  const layout = listed ? remove(workspace.layout, next.id) : workspace.layout;
  const inSlot = contains(layout, endedId);
  return {
    ...workspace,
    ...(herdrId ? { herdrId } : {}),
    panels: workspace.panels
      .filter((panel) => panel.id !== next.id || panel.id === endedId)
      .map((panel) =>
        panel.id === endedId
          ? { ...next, companion: panel.companion ?? next.companion }
          : panel,
      ),
    layout: inSlot
      ? replaceLeaf(layout, endedId, next.id)
      : layout
        ? split(layout, leaf(next.id), "row", 0.65)
        : leaf(next.id),
  };
}

/** The line a pane is typed to start an agent. With values to source,
 * the agent runs in a subshell: the values (a token, a key, the project's
 * secrets) live in the agent's process only, and are gone from the pane's
 * shell when it exits. */
export function agentLine(prefix: string, command: string, settings = "") {
  return prefix ? `(${prefix}exec ${command}${settings})` : command + settings;
}

/** True for a host-backed workspace with no live session: every session panel
 * of it ended, or it has none left. */
export function isVanished(workspace: Workspace): boolean {
  return (
    Boolean(workspace.connection) &&
    !workspace.panels.some((panel) => panel.sessionId && !panel.ended)
  );
}

/** The daemon call that renames a session-bound panel, or null when the title
 * is local only: an ended panel has no session to tell. */
export function renameRequest(
  owner: Workspace,
  panel: Panel,
  title: string,
  defaultEndpoint: string,
): { host: string; patch: { id: string; title: string } } | null {
  if (!panel.sessionId || panel.ended) return null;
  return {
    host: daemonHost(owner.connection || defaultEndpoint),
    patch: { id: panel.sessionId, title },
  };
}

/** The daemon call that ends a panel's session gracefully, or null when no
 * live session is bound to it. */
export function closeRequest(
  owner: Workspace,
  panel: Panel,
  defaultEndpoint: string,
): { host: string; id: string; graceful: true } | null {
  if (!panel.sessionId || panel.ended) return null;
  return {
    host: daemonHost(owner.connection || defaultEndpoint),
    id: panel.sessionId,
    graceful: true,
  };
}

/** What Reopen asks the launch entry for: a new session in the ended panel's
 * slot, resuming the agent's own session when the daemon reported one. The
 * workspace id is the daemon group (one id for both). */
export function reopenRequest(
  owner: Workspace,
  ended: Panel,
  operationId: string,
  defaultEndpoint: string,
): SessionLaunchRequest {
  const agent = ended.kind === "agent";
  return {
    operationId,
    endpoint: owner.connection || defaultEndpoint,
    cwd: owner.cwd,
    label: owner.name,
    kind: agent ? "agent" : "terminal",
    agent: agent ? ended.agent || "claude" : undefined,
    modelProfileId: ended.modelProfileId,
    claudeAccountId: ended.claudeAccountId,
    codexAccountId: ended.codexAccountId,
    workspaceId: owner.id,
    ...(agent && ended.agentSession ? { resume: ended.agentSession } : {}),
    ...(ended.launchError && !ended.ended && ended.sessionId
      ? { paneId: ended.sessionId }
      : {}),
    restore: true,
  };
}

/** The workspace that currently owns a panel id, wherever it lives - not
 * necessarily the active one. Used by anything that acts on a panel by id
 * (closePanel, renamePanel) so the action always reaches the pane's real
 * owner instead of silently no-op'ing against whichever workspace happens to
 * be active. */
export function findPanelOwner(
  workspaces: Workspace[],
  panelId: string,
): Workspace | undefined {
  return workspaces.find((workspace) =>
    workspace.panels.some((panel) => panel.id === panelId),
  );
}

/** A pane is a view, a thread is a conversation. Closing the view drops it from
 * the layout; session panels and chat threads stay in `panels` so the session and
 * the transcript survive. */
export function removePanel(workspace: Workspace, panel: Panel): Workspace {
  return {
    ...workspace,
    layout: remove(workspace.layout, panel.id),
    panels:
      panel.sessionId || panel.kind === "chat"
        ? workspace.panels
        : workspace.panels.filter((item) => item.id !== panel.id),
  };
}

/** Drops panels that a background operation ended. Applied to whatever the
 * workspace list is *now*, so it stays correct if the user switched workspaces
 * while the requests were in flight. Chat threads keep their transcript and
 * only leave the layout. */
export function removeClosedPanels(
  workspaces: Workspace[],
  closed: Set<string>,
): Workspace[] {
  if (!closed.size) return workspaces;
  let changed = false;
  const next = workspaces.map((workspace) => {
    if (!workspace.panels.some((panel) => closed.has(panel.id)))
      return workspace;
    changed = true;
    return {
      ...workspace,
      panels: workspace.panels.filter(
        (panel) => !closed.has(panel.id) || panel.kind === "chat",
      ),
      layout: [...closed].reduce(
        (tree, id) => remove(tree, id),
        workspace.layout,
      ),
    };
  });
  return changed ? next : workspaces;
}

/** Ending a session removes its pane but keeps the workspace project. */
export function removeClosedSessions(
  workspaces: Workspace[],
  closed: Set<string>,
): Workspace[] {
  return removeClosedPanels(workspaces, closed);
}

/** Center drop swaps the two panels; an edge drop re-inserts the source next to
 * the target. Anything that no longer matches the live layout is a no-op. */
export function movePanel(
  layout: Layout | null,
  source: string,
  target: string,
  edge: string,
): Layout | null {
  if (!layout || !contains(layout, source) || !contains(layout, target))
    return layout;
  if (edge === "center") return swap(layout, source, target);
  const without = remove(layout, source);
  return without ? insert(without, target, source, edge) : leaf(source);
}

export function tidyWorkspace(workspace: Workspace): Workspace {
  return {
    ...workspace,
    layout: tidy(codePanels(workspace).map((panel) => panel.id)),
  };
}

/** Keeps the current selection and zoom pointing at panels that still exist.
 * `groupIds` widens "still exists" to every code panel across an active
 * merge group's members (C2: selection works across members) - without it,
 * a pane belonging to a different member than the active workspace reads as
 * gone and the active workspace's own first panel is selected instead. */
export function fixSelection(
  workspace: Workspace,
  selected: string,
  zoomed: string | null,
  groupIds?: string[],
): { selected: string; zoomed: string | null } {
  const panels = codePanels(workspace);
  const groupAlive = groupIds ? new Set(groupIds) : null;
  const alive = (id: string) =>
    panels.some((panel) => panel.id === id) || !!groupAlive?.has(id);
  return {
    selected: selected && !alive(selected) ? panels[0]?.id || "" : selected,
    zoomed: zoomed && !alive(zoomed) ? null : zoomed,
  };
}

/** Every code panel id across a merge group's members, in member order -
 * used to tidy or reconcile the group's combined layout, and to widen
 * `fixSelection` across the group. Only a panel still in its own member's
 * layout counts: "hide only" (hidePanel in useWorkspaces.ts) drops a pane
 * from its owner's layout but leaves it in `panels` so the session survives,
 * exactly like single-workspace behaviour - counting it here anyway would
 * have `reconcileGroupLayout` read it as "gained" and append it straight
 * back into the merged canvas. */
export function groupPanelIds(group: MergeGroup): string[] {
  return group.members.flatMap((member) =>
    codePanels(member.workspace)
      .filter((panel) => contains(member.workspace.layout, panel.id))
      .map((panel) => panel.id),
  );
}

/** A tidy layout of every code panel across a merge group's members. */
export function tidyGroupLayout(group: MergeGroup): Layout | null {
  return tidy(groupPanelIds(group));
}

export type MergedPane = {
  panel: Panel;
  cwd: string;
  socket: string;
  endpoint?: string;
  hostLabel: string;
};

/** Every code panel across a merge group's members, each resolved to its own
 * owner's cwd, endpoint/connection and member label (C1) - the same
 * fields `PanelHost` computes inline for the single active workspace today,
 * generalized to each member. `appSocket` is the app's default connection,
 * the fallback `activeEndpoint` uses for the active workspace. */
export function resolveGroupPanes(
  group: MergeGroup,
  profiles: ConnectionProfile[],
  appSocket: string,
): MergedPane[] {
  return group.members.flatMap((member) =>
    codePanels(member.workspace).map((panel) => ({
      panel,
      cwd: panel.filesTarget?.root || member.workspace.cwd,
      socket: member.workspace.connection || appSocket,
      endpoint: member.workspace.connection,
      hostLabel: memberLabel(group, member, profiles),
    })),
  );
}

/** Reconciles a merge group's stored combined layout against its members'
 * current code panel ids: a member's panels change on their own (a session
 * is launched or closed), so a pane can appear or vanish between renders
 * here - a dropped id leaves the layout, a new one is appended beside the
 * rest. Falls back to `tidy` the first time, or once nothing from
 * the stored layout survives. */
export function reconcileGroupLayout(
  stored: Layout | null | undefined,
  panelIds: string[],
): Layout | null {
  const known = new Set(panelIds);
  let layout: Layout | null = stored ?? null;
  for (const id of leafIds(layout))
    if (!known.has(id)) layout = remove(layout, id);
  const present = new Set(leafIds(layout));
  const added = panelIds.filter((id) => !present.has(id));
  if (!layout) return tidy(added);
  return added.reduce(
    (tree, id) => split(tree, leaf(id), "column", 0.7),
    layout,
  );
}

const DEFAULT_TERMINAL_TITLES = new Set([
  "zsh",
  "Claude Code",
  "Codex",
  "Gemini CLI",
  "Cursor Agent",
]);

/** Follows the agent a terminal is running, but only while the panel still
 * carries a default title - a name the user typed is never overwritten. */
export function retitleTerminal(
  panel: Panel,
  agent: string | null | undefined,
): Panel {
  return {
    ...panel,
    agent: agent || undefined,
    title:
      panel.kind === "terminal" && DEFAULT_TERMINAL_TITLES.has(panel.title)
        ? agent
          ? agentTitle(agent)
          : "zsh"
        : panel.title,
  };
}

/** The workspace a project already has on a host: the one at its path,
 * or the one a start just made (before the host's listing catches up). A
 * session for the project is one more panel in it, never another workspace. */
export function findHostWorkspace(
  workspaces: Workspace[],
  defaultEndpoint: string,
  endpoint: string,
  cwd: string,
  madeId?: string,
): Workspace | undefined {
  return workspaces.find(
    (w) =>
      w.connection &&
      !isVanished(w) &&
      (w.connection || defaultEndpoint) === endpoint &&
      (w.cwd === cwd || w.id === madeId),
  );
}

export const COMPANION_RATIO = { min: 0.25, max: 0.75, fallback: 0.5 };

export function companionRatio(ratio: number | undefined): number {
  return typeof ratio === "number" && Number.isFinite(ratio)
    ? Math.min(COMPANION_RATIO.max, Math.max(COMPANION_RATIO.min, ratio))
    : COMPANION_RATIO.fallback;
}

function mapPanel(
  workspaces: Workspace[],
  panelId: string,
  update: (panel: Panel) => Panel,
): Workspace[] {
  return workspaces.map((w) =>
    w.panels.some((p) => p.id === panelId)
      ? {
          ...w,
          panels: w.panels.map((p) => (p.id === panelId ? update(p) : p)),
        }
      : w,
  );
}

/** An agent's open signal: the pane's companion shows `target` with `args`
 * merged over what it already held (the view state stored beside the file),
 * and is open again if it was hidden. A different surface starts afresh. */
export function openCompanion(
  workspaces: Workspace[],
  panelId: string,
  target: { extensionId: string; surfaceId: string },
  args: Record<string, string>,
): Workspace[] {
  return mapPanel(workspaces, panelId, (panel) => {
    const old = panel.companion;
    const same =
      old?.extensionId === target.extensionId &&
      old.surfaceId === target.surfaceId;
    return {
      ...panel,
      companion: {
        ...target,
        args: { ...(same ? old.args : {}), ...args },
        open: true,
        ...(same && old.ratio !== undefined ? { ratio: old.ratio } : {}),
      },
    };
  });
}

/** Changes a companion: `args` merge into its args, `ratio` is clamped. A pane
 * without a companion is left alone. */
export function patchCompanion(
  workspaces: Workspace[],
  panelId: string,
  patch: { args?: Record<string, string>; open?: boolean; ratio?: number },
): Workspace[] {
  return mapPanel(workspaces, panelId, (panel) =>
    panel.companion
      ? {
          ...panel,
          companion: {
            ...panel.companion,
            ...(patch.open === undefined ? {} : { open: patch.open }),
            ...(patch.ratio === undefined
              ? {}
              : { ratio: companionRatio(patch.ratio) }),
            args: { ...panel.companion.args, ...patch.args },
          },
        }
      : panel,
  );
}
