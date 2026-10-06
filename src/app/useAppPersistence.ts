import { useEffect, useMemo, useRef } from "react";
import {
  flushWorkspaceState,
  saveWorkspaceState,
  type Saved,
} from "../workspaceState.ts";

/** How long a burst of changes waits before it is written. Short enough that
 * a kill a moment later loses almost nothing, long enough that dragging a
 * split does not write on every frame. */
const WRITE_DELAY_MS = 500;

/** Commits the session to the workspace snapshot: debounced while the app is
 * running, synchronously when the window goes away. The fields are compared
 * one by one, so an unrelated re-render does not write. */
export function useAppPersistence(
  state: Saved,
  notify: (text: string) => void,
) {
  const {
    workspaces,
    activeId,
    routines,
    fontScale,
    mode,
    tabMode,
    section,
    selected,
    zoomed,
    sidebar,
    route,
    workspaceGrouping,
    closedProjects,
    views,
    mergedLayouts,
    agentTabs,
    agentFocus,
    chatFocus,
  } = state;
  const snapshot = useMemo<Saved>(
    () => ({
      workspaces,
      activeId,
      routines,
      fontScale,
      mode,
      tabMode,
      section,
      selected,
      zoomed,
      sidebar,
      route,
      workspaceGrouping,
      closedProjects,
      views,
      mergedLayouts,
      agentTabs,
      agentFocus,
      chatFocus,
    }),
    [
      workspaces,
      activeId,
      routines,
      fontScale,
      mode,
      tabMode,
      section,
      selected,
      zoomed,
      sidebar,
      route,
      workspaceGrouping,
      closedProjects,
      views,
      mergedLayouts,
      agentTabs,
      agentFocus,
      chatFocus,
    ],
  );
  const latest = useRef(snapshot);
  latest.current = snapshot;
  useEffect(() => {
    const failed = () => notify("Could not save the workspace to disk.");
    const timer = window.setTimeout(() => {
      try {
        void saveWorkspaceState(snapshot)?.catch(failed);
      } catch {
        failed();
      }
    }, WRITE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [snapshot, notify]);
  useEffect(() => {
    const flush = () => {
      try {
        flushWorkspaceState(latest.current);
      } catch {
        /* the window is closing; the last debounced write stands */
      }
    };
    window.addEventListener("beforeunload", flush);
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("beforeunload", flush);
      window.removeEventListener("pagehide", flush);
    };
  }, []);
}
