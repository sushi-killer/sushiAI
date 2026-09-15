import { useEffect, useRef, useState } from "react";
import type { ProjectView, ProjectViews, Saved } from "../workspaceState.ts";
import type { WorkspaceController } from "./useWorkspaces.ts";

const DEFAULT_VIEW: ProjectView = { tabMode: false, zoomed: null };

export function viewFor(views: ProjectViews, key: string): ProjectView {
  return views[key] ?? DEFAULT_VIEW;
}

/** Records `view` as the way `key` is looked at; returns the same object when
 * nothing changed, so a re-render never rewrites storage. */
export function rememberView(
  views: ProjectViews,
  key: string,
  view: ProjectView,
): ProjectViews {
  const known = views[key];
  if (known?.tabMode === view.tabMode && known.zoomed === view.zoomed)
    return views;
  return { ...views, [key]: view };
}

/** Decides what a key change should zoom to. `switchWorkspace` always resets
 * `zoomed` to `null` before anything else runs, so a non-null value seen here
 * can only come from an explicit "show this pane" set in the same batch
 * (App.tsx's selectHostPane, SessionsDialog's onShow, the blocked-panel
 * notification) - that intent wins over the project's own stored zoom.
 * Otherwise there was no explicit pane to show, so the stored zoom restores
 * as before. */
export function resolveZoomOnKeyChange(
  zoomed: string | null,
  storedZoomed: string | null,
): string | null {
  return zoomed !== null ? zoomed : storedZoomed;
}

/** Tabs-or-split and the maximized pane belong to a project, not to the app:
 * one project wants many panes side by side, another a single focused one.
 * `key` is the merge group's id for a merged project (its members share one
 * canvas) and the workspace id otherwise. While a project stays open its
 * current view is recorded under its key; opening another restores that
 * one's own. */
// ponytail: views are never pruned; drop entries of long-gone workspaces if the map ever matters in size.
export function useProjectView(
  key: string,
  ws: Pick<WorkspaceController, "zoomed" | "setZoomed">,
  saved: Saved | null,
) {
  const [tabMode, setTabMode] = useState(saved?.tabMode || false);
  const [views, setViews] = useState<ProjectViews>(saved?.views ?? {});
  const { zoomed, setZoomed } = ws;
  const shown = useRef(key);
  const viewsRef = useRef(views);
  viewsRef.current = views;
  useEffect(() => {
    if (shown.current === key) {
      setViews((all) => rememberView(all, key, { tabMode, zoomed }));
      return;
    }
    shown.current = key;
    const next = viewFor(viewsRef.current, key);
    setTabMode(next.tabMode);
    setZoomed(resolveZoomOnKeyChange(zoomed, next.zoomed));
  }, [key, tabMode, zoomed, setZoomed]);
  return { tabMode, setTabMode, views };
}
