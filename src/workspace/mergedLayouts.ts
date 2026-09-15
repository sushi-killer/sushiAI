import { useCallback, useEffect, useState } from "react";
import { activeMergeGroup, memberLabel } from "../app/workspaceMerge.ts";
import type { ConnectionProfile, Layout } from "../types";
import type { ProjectGit } from "../app/useProjectGit.ts";
import {
  groupPanelIds,
  reconcileGroupLayout,
  resolveGroupPanes,
} from "./workspace-actions.ts";
import type { MergedPane } from "./workspace-actions.ts";
import type { WorkspaceController } from "./useWorkspaces.ts";

const STORAGE = "sushiai.mergedLayouts.v1";

type StoredGroupLayouts = Record<string, Layout>;

function load(): StoredGroupLayouts {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE) || "{}");
    return value && typeof value === "object" && !Array.isArray(value)
      ? (value as StoredGroupLayouts)
      : {};
  } catch {
    return {};
  }
}

export type MergedCanvas = {
  group: ReturnType<typeof activeMergeGroup>;
  panes: MergedPane[];
  layout: Layout | null;
  hostLabel: string | undefined;
};

/** Draws a merged project's panes from every host on one canvas (flat mode
 * only). A merge group has no workspace of its own to keep a combined
 * layout in - Herdr polls rewrite each member's own `layout` independently
 * every few seconds (see reconcileGroupLayout in workspace-actions.ts) - so
 * it is persisted here instead, keyed by the merge group's stable id, and
 * survives restart the same way a single workspace's layout does. Assigns
 * `ws.groupRef` synchronously each render so `drop`/`resizeSplit`/`tidy` and
 * the selection effect in useWorkspaces.ts can act on it. */
export function useMergedCanvas(
  ws: WorkspaceController,
  projectGit: Record<string, ProjectGit>,
  connectionProfiles: ConnectionProfile[],
  workspaceGrouping: "grouped" | "flat",
  socket: string,
): MergedCanvas {
  const [stored, setStored] = useState<StoredGroupLayouts>(() =>
    typeof localStorage === "undefined" ? {} : load(),
  );
  useEffect(() => {
    try {
      localStorage.setItem(STORAGE, JSON.stringify(stored));
    } catch {
      /* storage full or unavailable: the layout still works this session */
    }
  }, [stored]);
  const setGroupLayout = useCallback(
    (groupId: string, layout: Layout | null) =>
      setStored((current) =>
        layout
          ? { ...current, [groupId]: layout }
          : Object.fromEntries(
              Object.entries(current).filter(([id]) => id !== groupId),
            ),
      ),
    [],
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
  const activeMember = group?.members.find(
    (m) => m.workspace.id === ws.active.id,
  );
  return {
    group,
    panes: group ? resolveGroupPanes(group, connectionProfiles, socket) : [],
    layout,
    hostLabel:
      group && activeMember
        ? memberLabel(group, activeMember, connectionProfiles)
        : undefined,
  };
}
