import type { Layout, Panel, Snapshot, Workspace } from "./types";

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
  const endpointKey = connection.startsWith("ssh:") ? connection : "local";
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
  function buildWorkspace(workspaceId: string): Workspace {
    const workspace = bySnapshotId.get(workspaceId)!;
    const old = existing.get(workspace.workspace_id);
    const remotePanes = panesByWorkspace.get(workspace.workspace_id) || [];
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
        id: existingPanel?.id || `herdr:${endpointKey}:${pane.pane_id}`,
        kind: pane.agent ? "agent" : "terminal",
        title:
          pane.label ||
          (pane.agent
            ? agentTitle(pane.agent)
            : pane.terminal_title_stripped || "zsh"),
        herdrId: pane.pane_id,
        agent: pane.agent,
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
    const extras = (old?.panels || []).filter((panel) => !panel.herdrId);
    const allPanels = [...panels, ...gone, ...extras];
    let layout = old ? old.layout : tidy(allPanels.map((panel) => panel.id));
    if (old) {
      const oldPanelIds = new Set(old.panels.map((panel) => panel.id));
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
    // workspace never falls back to this Mac's home.
    const cwd =
      (old?.cwd && old.cwd !== systemHome ? old.cwd : "") ||
      remotePanes.find((pane) => pane.cwd)?.cwd ||
      workspace.worktree?.checkout_path ||
      (endpointKey === "local" ? systemHome : old?.cwd || "");
    return {
      id: old?.id || `herdr:${endpointKey}:${workspace.workspace_id}`,
      connection,
      herdrId: workspace.workspace_id,
      name: workspace.label,
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
  // A workspace the host dropped is kept so it can be reopened - unless the
  // host already has a live one at the same folder: then it is a stale
  // double of that one, and a list of them is what a few restarts left.
  const ownDropped = (workspace: Workspace) =>
    Boolean(workspace.herdrId) &&
    workspace.connection === connection &&
    !bySnapshotId.has(workspace.herdrId!);
  const all = [...nextBase, ...brandNew];
  const liveFolders = new Set(
    all
      .filter(
        (workspace) =>
          workspace.herdrId &&
          workspace.connection === connection &&
          !ownDropped(workspace),
      )
      .map((workspace) => workspace.cwd),
  );
  const next = all.filter(
    (workspace) => !(ownDropped(workspace) && liveFolders.has(workspace.cwd)),
  );
  return sameWorkspaces(current, next) ? current : next;
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
