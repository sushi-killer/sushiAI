import { useCallback, useEffect, useState } from "react";
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

const STORAGE = "sushiai.mergedLayouts.v1";

type StoredGroupLayouts = Record<string, Layout>;

/** localStorage is outside the app's control - a stale format, a hand-edited
 * value or a future version this build doesn't know could all leave
 * something that isn't a real layout tree behind. `LayoutView` (and anything
 * else here) assumes a `leaf`/`split` shape without checking, so a malformed
 * entry crashes the canvas instead of just being dropped. */
export function isValidLayout(value: unknown): value is Layout {
  if (!value || typeof value !== "object") return false;
  const node = value as { type?: unknown };
  if (node.type === "leaf")
    return typeof (node as { id?: unknown }).id === "string";
  if (node.type === "split") {
    const split = node as {
      id?: unknown;
      axis?: unknown;
      ratio?: unknown;
      a?: unknown;
      b?: unknown;
    };
    return (
      typeof split.id === "string" &&
      (split.axis === "row" || split.axis === "column") &&
      typeof split.ratio === "number" &&
      isValidLayout(split.a) &&
      isValidLayout(split.b)
    );
  }
  return false;
}

function load(): StoredGroupLayouts {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE) || "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).filter(([, layout]) =>
        isValidLayout(layout),
      ),
    ) as StoredGroupLayouts;
  } catch {
    return {};
  }
}

export type MergedCanvas = {
  group: ReturnType<typeof activeMergeGroup>;
  panes: MergedPane[];
  layout: Layout | null;
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
  return {
    group,
    panes: group ? resolveGroupPanes(group, connectionProfiles, socket) : [],
    layout,
  };
}
