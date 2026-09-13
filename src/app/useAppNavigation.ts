import { useCallback, useEffect, useMemo, useReducer } from "react";
import {
  coreSection,
  extensionSection,
  navigationReducer,
  openExtensionRef,
  restoreNavigation,
  routeOf,
  sectionName,
  shouldCloseExtension,
  type ExtensionRouteState,
  type SectionRef,
} from "./navigation.ts";
import type { CoreMode } from "../extensions/routes.ts";
import type { Saved } from "../workspaceState.ts";

export function useAppNavigation(
  saved: Saved | null,
  surfaces?: {
    ready: boolean;
    hasSurface(target: ExtensionRouteState): boolean;
  },
) {
  const [state, dispatch] = useReducer(
    navigationReducer,
    saved,
    restoreNavigation,
  );
  const showWorkspace = useCallback(
    () => dispatch({ type: "showWorkspace" }),
    [],
  );
  const ready = surfaces?.ready ?? false;
  const hasSurface = surfaces?.hasSurface;
  useEffect(() => {
    // A saved extension page whose surface is gone would otherwise leave the
    // shell on a view that no navigation control marks as active.
    if (!ready || !hasSurface) return;
    if (shouldCloseExtension(state, hasSurface)) showWorkspace();
  }, [ready, hasSurface, state, showWorkspace]);
  return {
    mode: state.mode,
    /** The open page, whoever contributed it. */
    section: state.section,
    /** The core section's name, or "" - what the core pages switch on. */
    sectionName: sectionName(state.section),
    extension: openExtensionRef(state.section),
    route: useMemo(() => routeOf(state), [state]),
    setMode: useCallback(
      (mode: CoreMode) => dispatch({ type: "setMode", mode }),
      [],
    ),
    toggleSection: useCallback(
      (section: SectionRef) => dispatch({ type: "toggleSection", section }),
      [],
    ),
    toggleCoreSection: useCallback(
      (name: string) =>
        dispatch({ type: "toggleSection", section: coreSection(name) }),
      [],
    ),
    openExtension: useCallback(
      (extension: ExtensionRouteState) =>
        dispatch({
          type: "toggleSection",
          section: extensionSection(extension.extensionId, extension.surfaceId),
        }),
      [],
    ),
    showWorkspace,
  };
}
