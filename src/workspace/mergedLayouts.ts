import { useCallback } from "react";
import { activeMergeGroup } from "../app/workspaceMerge.ts";
import type { ConnectionProfile, Layout } from "../types";
import type { ProjectGit } from "../app/useProjectGit.ts";
import {
  groupPanelIds,
  reconcileGroupLayout,
  resolveGroupPanes,
} from "./workspace-actions.ts";
import type { MergedPane } from "./workspace-actions.ts";
import type { WorkspaceController } from "./useWorkspaces.ts";
import type { SessionState } from "../app/useSessionState.ts";

export type MergedCanvas = {
  group: ReturnType<typeof activeMergeGroup>;
  panes: MergedPane[];
  layout: Layout | null;
};

/** Draws a merged project's panes from every host on one canvas (flat mode
 * only). A merge group has no workspace of its own to keep a combined
 * layout in - each member's panels change on their own (see
 * reconcileGroupLayout in workspace-actions.ts) - so it is kept in the
 * workspace snapshot instead (`mergedLayouts`, owned by
 * App and passed in), keyed by the merge group's stable id, and survives
 * restart the same way a single workspace's layout does. Assigns
 * `ws.groupRef` synchronously each render so `drop`/`resizeSplit`/`tidy` and
 * the selection effect in useWorkspaces.ts can act on it. */
export function useMergedCanvas(
  ws: WorkspaceController,
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
  workspaceGrouping: "grouped" | "flat",
  { mergedLayouts: stored, setMergedLayouts: setStored }: SessionState,
): MergedCanvas {
  const setGroupLayout = useCallback(
    (groupId: string, layout: Layout | null) =>
      setStored((current) =>
        layout
          ? { ...current, [groupId]: layout }
          : Object.fromEntries(
              Object.entries(current).filter(([id]) => id !== groupId),
            ),
      ),
    [setStored],
  );
  const group = activeMergeGroup(
    ws.workspaces,
    ws.active,
    projectGit,
    connectionProfiles,
    workspaceGrouping,
  );
  const layout = group
    ? reconcileGroupLayout(stored[group.id], groupPanelIds(group))
    : null;
  ws.groupRef.current = group
    ? { group, layout, setLayout: (next) => setGroupLayout(group.id, next) }
    : undefined;
  return {
    group,
    panes: group ? resolveGroupPanes(group, connectionProfiles) : [],
    layout,
  };
}
