import {
  contains,
  insert,
  leaf,
  leafIds,
  remove,
  split,
  swap,
  tidy,
} from "../layout.ts";
import { agentTitle } from "../app/agent-title.ts";
import { memberLabel } from "../app/workspaceMerge.ts";
import type { MergeGroup } from "../app/workspaceMerge.ts";
import { codePanels } from "../workspaceState.ts";
import type { ConnectionProfile, Layout, Panel, Workspace } from "../types";

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

/** A pane is a view, a thread is a conversation. Closing the view drops it from
 * the layout; Herdr panes and chat threads stay in `panels` so the session and
 * the transcript survive. */
export function removePanel(workspace: Workspace, panel: Panel): Workspace {
  return {
    ...workspace,
    layout: remove(workspace.layout, panel.id),
    panels:
      panel.herdrId || panel.kind === "chat"
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
 * `fixSelection` across the group. */
export function groupPanelIds(group: MergeGroup): string[] {
  return group.members.flatMap((member) =>
    codePanels(member.workspace).map((panel) => panel.id),
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
 * owner's cwd, Herdr endpoint/connection and member label (C1) - the same
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
 * current code panel ids: Herdr polls rewrite each member's own `layout`
 * independently every few seconds, so a pane can appear or vanish between
 * renders here - a dropped id leaves the layout, a new one is appended
 * beside the rest. Falls back to `tidy` the first time, or once nothing from
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

/** Panels whose agent is waiting on the user, with the workspace each belongs
 * to. Drives both the bell dot and the notifications list. */
export function blockedPanels(
  workspaces: Workspace[],
): { workspace: Workspace; panel: Panel }[] {
  return workspaces.flatMap((workspace) =>
    workspace.panels
      .filter((panel) => panel.status === "blocked")
      .map((panel) => ({ workspace, panel })),
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
