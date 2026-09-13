import {
  contains,
  insert,
  leaf,
  remove,
  split,
  swap,
  tidy,
} from "../layout.ts";
import { agentTitle } from "../app/agent-title.ts";
import { codePanels } from "../workspaceState.ts";
import type { Layout, Panel, Workspace } from "../types";

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

/** Keeps the current selection and zoom pointing at panels that still exist. */
export function fixSelection(
  workspace: Workspace,
  selected: string,
  zoomed: string | null,
): { selected: string; zoomed: string | null } {
  const panels = codePanels(workspace);
  const alive = (id: string) => panels.some((panel) => panel.id === id);
  return {
    selected: selected && !alive(selected) ? panels[0]?.id || "" : selected,
    zoomed: zoomed && !alive(zoomed) ? null : zoomed,
  };
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
