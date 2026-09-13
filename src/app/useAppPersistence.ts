import { useEffect } from "react";
import { saveWorkspaceState, type Saved } from "../workspaceState.ts";

/** Commits the session before a reload or a background snapshot arrives. The
 * fields are compared one by one, so an unrelated re-render does not write. */
export function useAppPersistence(
  state: Saved,
  notify: (text: string) => void,
) {
  const {
    workspaces,
    activeId,
    socket,
    routines,
    fontScale,
    mode,
    tabMode,
    section,
    selected,
    zoomed,
    sidebar,
    route,
  } = state;
  useEffect(() => {
    try {
      saveWorkspaceState({
        workspaces,
        activeId,
        socket,
        routines,
        fontScale,
        mode,
        tabMode,
        section,
        selected,
        zoomed,
        sidebar,
        route,
      });
    } catch {
      notify("Storage is full. Clear older chat history.");
    }
  }, [
    workspaces,
    activeId,
    socket,
    routines,
    fontScale,
    mode,
    tabMode,
    section,
    selected,
    zoomed,
    sidebar,
    route,
    notify,
  ]);
}
