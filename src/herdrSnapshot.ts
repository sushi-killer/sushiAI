import type { Layout, Panel, Snapshot, Workspace } from "./types";
import { herdrWorkspaceKey } from "./herdrIdentity.ts";

const leaf = (id: string): Layout => ({ type: "leaf", id });
const splitLayout = (
  a: Layout,
  b: Layout,
  axis: "row" | "column",
  ratio = 0.5,
): Layout => ({ type: "split", id: crypto.randomUUID(), axis, ratio, a, b });
/** Every leaf id, walked without recursion: a layout can be thousands deep. */
function leafSet(layout: Layout | null): Set<string> {
  const ids = new Set<string>();
  const stack = layout ? [layout] : [];
  while (stack.length) {
    const node = stack.pop()!;
    if (node.type === "leaf") ids.add(node.id);
    else stack.push(node.a, node.b);
  }
  return ids;
}
/** A Herdr pane its host no longer lists. It keeps its slot and its saved
 * state; the pane shows "Session ended" until it is reopened or closed. */
function endPane(panel: Panel): Panel {
  if (panel.ended) return panel;
  const next: Panel = { ...panel, ended: true };
  delete next.status;
  return next;
}
function tidy(ids: string[]): Layout | null {
  if (!ids.length) return null;
  if (ids.length === 1) return leaf(ids[0]);
  const first = ids[0];
  let rest = leaf(ids[ids.length - 1]);
  for (let index = ids.length - 2; index > 0; index -= 1) {
    rest = splitLayout(
      leaf(ids[index]),
      rest,
      "column",
      1 / (ids.length - index),
    );
  }
  return splitLayout(leaf(first), rest, "row", ids.length > 2 ? 0.53 : 0.5);
}

function sameValue(
  a: unknown,
  b: unknown,
  seen = new WeakMap<object, object>(),
  depth = 0,
): boolean {
  if (Object.is(a, b)) return true;
  if (depth > 200) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((value, index) => sameValue(value, b[index], seen, depth + 1))
    );
  }
  if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
  if (seen.get(a) === b) return true;
  seen.set(a, b);
  const aRecord = a as Record<string, unknown>;
  const bRecord = b as Record<string, unknown>;
  const aKeys = Object.keys(aRecord);
  const bKeys = Object.keys(bRecord);
  return (
    aKeys.length === bKeys.length &&
    aKeys.every(
      (key) =>
        Object.prototype.hasOwnProperty.call(bRecord, key) &&
        sameValue(aRecord[key], bRecord[key], seen, depth + 1),
    )
  );
}

function sameWorkspaces(a: Workspace[], b: Workspace[]): boolean {
  return sameValue(a, b);
}

/** A workspace its host no longer lists stays in the list, every Herdr pane
 * of it ended. Returns the same object when nothing changes. */
function endWorkspace(workspace: Workspace): Workspace {
  const panels = workspace.panels.map((panel) =>
    panel.herdrId ? endPane(panel) : panel,
  );
  return panels.every((panel, index) => panel === workspace.panels[index])
    ? workspace
    : { ...workspace, panels };
}

export function reconcileHerdrWorkspaces(
  current: Workspace[],
  snapshot: Snapshot,
  connection: string,
  systemHome = "",
): Workspace[] {
  const existing = new Map(
    current
      .filter(
        (workspace) => workspace.herdrId && workspace.connection === connection,
      )
      .map((workspace) => [workspace.herdrId as string, workspace]),
  );
  const panesByWorkspace = new Map<string, Snapshot["panes"]>();
  for (const pane of snapshot.panes) {
    const panes = panesByWorkspace.get(pane.workspace_id) || [];
    panes.push(pane);
    panesByWorkspace.set(pane.workspace_id, panes);
  }

  const bySnapshotId = new Map(
    snapshot.workspaces.map((workspace) => [workspace.workspace_id, workspace]),
  );
  const adoptedProjectIds = new Set<string>();
  function buildWorkspace(workspaceId: string): Workspace {
    const workspace = bySnapshotId.get(workspaceId)!;
    const old = existing.get(workspace.workspace_id);
    const remotePanes = panesByWorkspace.get(workspace.workspace_id) || [];
    const cwdHint =
      workspace.worktree?.checkout_path ||
      remotePanes.find((pane) => pane.cwd)?.cwd;
    const preservedProject =
      !old && cwdHint
        ? current.find(
            (candidate) =>
              candidate.herdrId &&
              candidate.connection === connection &&
              !bySnapshotId.has(candidate.herdrId) &&
              candidate.panels.every((panel) => panel.kind === "chat") &&
              !adoptedProjectIds.has(candidate.id) &&
              candidate.cwd === cwdHint,
          )
        : undefined;
    if (preservedProject) adoptedProjectIds.add(preservedProject.id);
    const saved = old || preservedProject;
    const oldPanelsByHerdr = new Map(
      (old?.panels || [])
        .filter((panel) => panel.herdrId)
        .map((panel) => [panel.herdrId as string, panel]),
    );
    // Same reasoning as the workspace order above, one level down: keep each
    // pane where it already was and only append genuinely new ones.
    const byPaneId = new Map(remotePanes.map((pane) => [pane.pane_id, pane]));
    const orderedPaneIds = [
      ...[...oldPanelsByHerdr.keys()].filter((id) => byPaneId.has(id)),
      ...remotePanes
        .map((pane) => pane.pane_id)
        .filter((id) => !oldPanelsByHerdr.has(id)),
    ];
    const panels: Panel[] = orderedPaneIds.map((paneId) => {
      const pane = byPaneId.get(paneId)!;
      const existingPanel = oldPanelsByHerdr.get(pane.pane_id);
      const nextPanel: Panel = {
        ...(existingPanel || {}),
        id: existingPanel?.id || herdrWorkspaceKey(connection, pane.pane_id),
        kind:
          (existingPanel?.launchError && existingPanel.kind === "agent") ||
          pane.agent
            ? "agent"
            : "terminal",
        title:
          (existingPanel?.launchError ? existingPanel.title : undefined) ||
          pane.label ||
          (pane.agent
            ? agentTitle(pane.agent)
            : pane.terminal_title_stripped || "zsh"),
        herdrId: pane.pane_id,
        agent: existingPanel?.launchError ? existingPanel.agent : pane.agent,
        status: pane.agent_status,
      };
      delete nextPanel.ended;
      return existingPanel && sameValue(existingPanel, nextPanel)
        ? existingPanel
        : nextPanel;
    });
    const remotePanelIds = new Set(panels.map((panel) => panel.id));
    // A pane the host dropped stays in its layout slot, marked ended. One that
    // was already hidden from the layout has nothing to show, so it goes.
    const inLayout = leafSet(old?.layout ?? null);
    const gone = (old?.panels || [])
      .filter(
        (panel) =>
          panel.herdrId &&
          !remotePanelIds.has(panel.id) &&
          inLayout.has(panel.id),
      )
      .map(endPane);
    const extras = (old?.panels || preservedProject?.panels || []).filter(
      (panel) => !panel.herdrId,
    );
    const allPanels = [...panels, ...gone, ...extras];
    let layout = saved
      ? saved.layout
      : tidy(allPanels.map((panel) => panel.id));
    if (saved) {
      const oldPanelIds = new Set(saved.panels.map((panel) => panel.id));
      const addedPanels = panels.filter((panel) => !oldPanelIds.has(panel.id));
      if (addedPanels.length) {
        layout = layout
          ? addedPanels.reduce(
              (current, panel) =>
                splitLayout(current, leaf(panel.id), "column"),
              layout,
            )
          : tidy(addedPanels.map((panel) => panel.id));
      }
    }
    // A workspace keeps the folder it was opened at: a pane that `cd`s
    // elsewhere does not move it to another project. A remote host's
    // workspace never falls back to the local home.
    const cwd =
      workspace.worktree?.checkout_path ||
      (saved?.cwd && saved.cwd !== systemHome ? saved.cwd : "") ||
      remotePanes.find((pane) => pane.cwd)?.cwd ||
      (connection.startsWith("ssh:") ? old?.cwd || "" : systemHome);
    return {
      ...saved,
      id: saved?.id || herdrWorkspaceKey(connection, workspace.workspace_id),
      connection,
      herdrId: workspace.workspace_id,
      herdrTokens: workspace.tokens ?? {},
      name: preservedProject?.name ?? workspace.label,
      cwd,
      panels: allPanels,
      layout,
    };
  }
  // This app polls every connected host concurrently, each on its own timer
  // (see useHerdr), so a poll for any one connection must never move workspaces
  // belonging to a DIFFERENT connection, or this connection's own workspaces
  // to a new spot - only ever update them in place. Otherwise every
  // independent poll reordered the whole array: a flat (unsectioned) list
  // visibly reshuffled every few seconds, and the order-sensitive equality
  // check below saw a "change" (spurious re-renders) even when nothing about
  // any workspace had actually changed.
  const nextBase = current.map((workspace) => {
    if (!workspace.herdrId || workspace.connection !== connection)
      return workspace;
    return bySnapshotId.has(workspace.herdrId)
      ? buildWorkspace(workspace.herdrId)
      : endWorkspace(workspace);
  });
  const brandNew = [...bySnapshotId.keys()]
    .filter((id) => !existing.has(id))
    .map((id) => buildWorkspace(id));
  // A host shows a project's folder once. A workspace the host dropped is
  // kept so it can be reopened - unless the folder already has a workspace on
  // this host (a live one first, else an earlier dropped one): then it folds
  // into that one. Its chats, files and other non-Herdr panels move there;
  // its ended Herdr panes, which nothing can reopen twice, go. That is what a
  // few restarts and re-starts used to leave as a list of doubles.
  const all = [...nextBase, ...brandNew];
  const own = (workspace: Workspace) =>
    Boolean(workspace.herdrId && workspace.cwd) &&
    workspace.connection === connection;
  const live = (workspace: Workspace) => bySnapshotId.has(workspace.herdrId!);
  const home = new Map<string, Workspace>();
  for (const workspace of all.filter(own).filter(live))
    if (!home.has(workspace.cwd)) home.set(workspace.cwd, workspace);
  for (const workspace of all.filter(own))
    if (!home.has(workspace.cwd)) home.set(workspace.cwd, workspace);
  const moved = new Map<string, Panel[]>();
  const kept = all.filter((workspace) => {
    if (!own(workspace) || live(workspace)) return true;
    const target = home.get(workspace.cwd)!;
    if (target === workspace) return true;
    const extras = workspace.panels.filter(
      (panel) =>
        !panel.herdrId &&
        !target.panels.some((existingPanel) => existingPanel.id === panel.id),
    );
    if (extras.length)
      moved.set(target.id, [...(moved.get(target.id) || []), ...extras]);
    return false;
  });
  const next = kept.map((workspace) => {
    const extras = moved.get(workspace.id);
    return extras
      ? {
          ...workspace,
          panels: [...workspace.panels, ...extras],
          layout: extras.reduce<Layout | null>(
            (layout, panel) =>
              layout
                ? splitLayout(layout, leaf(panel.id), "column")
                : leaf(panel.id),
            workspace.layout,
          ),
        }
      : workspace;
  });
  const byId = new Map(next.map((workspace) => [workspace.id, workspace]));
  const currentIds = new Set(current.map((workspace) => workspace.id));
  const ordered = [
    ...current.flatMap((workspace) => {
      const nextWorkspace = byId.get(workspace.id);
      return nextWorkspace ? [nextWorkspace] : [];
    }),
    ...next.filter((workspace) => !currentIds.has(workspace.id)),
  ];
  return sameWorkspaces(current, ordered) ? current : ordered;
}

function agentTitle(agent: string): string {
  return (
    {
      claude: "Claude Code",
      codex: "Codex",
      gemini: "Gemini CLI",
      "cursor-agent": "Cursor Agent",
    }[agent] || agent
  );
}
