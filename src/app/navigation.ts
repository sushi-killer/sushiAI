import {
  coreRoute,
  extensionRoute,
  routeFromLegacy,
  SECTIONS,
  type CoreMode,
  type RouteRef,
} from "../extensions/routes.ts";
import type { Saved } from "../workspaceState.ts";

export type ExtensionRouteState = { extensionId: string; surfaceId: string };

/** Which page is open in the working area. A page an extension contributes is
 * one of these, not a state of its own: the shell has no way to tell it from
 * Dashboard or Skills, which is what keeps a manifest out of the chrome. */
export type SectionRef =
  | { kind: "core"; id: string }
  | ({ kind: "extension" } & ExtensionRouteState);

export type NavigationState = {
  mode: CoreMode;
  section: SectionRef | null;
};

export type NavigationAction =
  | { type: "setMode"; mode: CoreMode }
  | { type: "toggleSection"; section: SectionRef }
  | { type: "showWorkspace" };

const workspaceView: NavigationState = { mode: "Code", section: null };

export const coreSection = (id: string): SectionRef => ({ kind: "core", id });
export const extensionSection = (
  extensionId: string,
  surfaceId: string,
): SectionRef => ({ kind: "extension", extensionId, surfaceId });

export const sectionName = (section: SectionRef | null): string =>
  section?.kind === "core" ? section.id : "";

export const openExtensionRef = (
  section: SectionRef | null,
): ExtensionRouteState | null =>
  section?.kind === "extension"
    ? { extensionId: section.extensionId, surfaceId: section.surfaceId }
    : null;

export function restoreNavigation(saved: Saved | null): NavigationState {
  const mode: CoreMode = saved?.mode || "Code";
  if (
    saved?.route?.kind === "extension" &&
    saved.route.extensionId &&
    saved.route.targetSurfaceId
  )
    return {
      mode,
      section: extensionSection(
        saved.route.extensionId,
        saved.route.targetSurfaceId,
      ),
    };
  return {
    mode,
    section:
      saved?.section && SECTIONS.has(saved.section)
        ? coreSection(saved.section)
        : null,
  };
}

export function navigationReducer(
  state: NavigationState,
  action: NavigationAction,
): NavigationState {
  switch (action.type) {
    case "setMode":
      return { mode: action.mode, section: null };
    case "toggleSection": {
      const section = sameSection(state.section, action.section)
        ? null
        : action.section;
      // A page belongs to the Code shell. Agent and Chat fill the sidebar with
      // their own list, and opening a page unmounts the view that fills it -
      // leaving the sidebar blank if the mode were kept.
      return { mode: section ? "Code" : state.mode, section };
    }
    case "showWorkspace":
      return state.mode === "Code" && !state.section ? state : workspaceView;
  }
}

export function routeOf(state: NavigationState): RouteRef {
  if (state.section?.kind === "extension")
    return extensionRoute(state.section.extensionId, state.section.surfaceId);
  return state.section
    ? routeFromLegacy(state.mode, state.section.id)
    : coreRoute(state.mode);
}

/** True while the open page belongs to a surface that is no longer available,
 * which would otherwise leave the shell on a page no navigation control marks
 * as active. */
export function shouldCloseExtension(
  state: NavigationState,
  hasSurface: (extension: ExtensionRouteState) => boolean,
): boolean {
  const open = openExtensionRef(state.section);
  return Boolean(open) && !hasSurface(open!);
}

function sameSection(a: SectionRef | null, b: SectionRef | null): boolean {
  if (!a || !b) return a === b;
  if (a.kind !== b.kind) return false;
  return a.kind === "core"
    ? a.id === (b as { id: string }).id
    : a.extensionId === (b as ExtensionRouteState).extensionId &&
        a.surfaceId === (b as ExtensionRouteState).surfaceId;
}
