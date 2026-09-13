import type { ExtensionRegistry } from "./registry.ts";
import type { Panel } from "../types.ts";
import type {
  ExtensionSnapshot,
  NavigationContribution,
  NavigationPlacement,
  SurfaceContribution,
  SurfaceHost,
  WorkspaceActionContribution,
  WorkspaceActionPlacement,
} from "./types.ts";

export type CoreMode = "Agent" | "Code" | "Chat";
export type ExtensionTarget = { extensionId: string; surfaceId: string };
export type RouteRef = {
  kind: "core" | "extension";
  surfaceId: string;
  /** "page" is only ever read, never written: it is what builds of this branch
   * saved before a contributed page became an ordinary section. */
  presentation: "page" | "section" | "workspace";
  extensionId?: string;
  targetSurfaceId?: string;
};

export const SECTIONS = new Set([
  "Dashboard",
  "Sessions",
  "Routines",
  "Extensions",
  "Skills",
]);

export const coreRoute = (mode: CoreMode): RouteRef => ({
  kind: "core",
  surfaceId: `core.mode.${mode.toLowerCase()}`,
  presentation: "workspace",
});

export function routeFromLegacy(mode: CoreMode, section = ""): RouteRef {
  if (SECTIONS.has(section))
    return {
      kind: "core",
      surfaceId: `core.section.${section.toLowerCase()}`,
      presentation: "section",
    };
  return coreRoute(mode);
}

export function extensionRoute(
  extensionId: string,
  surfaceId: string,
): RouteRef {
  return {
    kind: "extension",
    surfaceId: `extension.${extensionId}.${surfaceId}`,
    presentation: "section",
    extensionId,
    targetSurfaceId: surfaceId,
  };
}

export function validRoute(value: unknown): value is RouteRef {
  if (!value || typeof value !== "object") return false;
  const route = value as Partial<RouteRef>;
  if (!route.surfaceId || typeof route.surfaceId !== "string") return false;
  if (!["page", "section", "workspace"].includes(route.presentation || ""))
    return false;
  if (route.kind === "core") return true;
  return (
    route.kind === "extension" &&
    typeof route.extensionId === "string" &&
    typeof route.targetSurfaceId === "string"
  );
}

/** Placements whose entry opens a full app page. A pane-only placement like
 * `panel.picker` is deliberately absent. */
const PAGE_PLACEMENTS: NavigationPlacement[] = [
  "mode.primary",
  "sidebar.primary",
  "dashboard.navigation",
  "sessions.navigation",
  "skills.navigation",
];

/** Whether a surface can be shown as a page at all: it must allow the host and
 * still be reachable from some active entry, or the page would have no control
 * to return to and would close itself on the next render.
 *
 * Every resolver below shares this one answer - they used to disagree, so a
 * sidebar entry pointing at a page opened and then immediately closed. */
export function pageReachable(
  registry: ExtensionRegistry,
  target: ExtensionTarget,
): boolean {
  const surface = activeSurface(registry, target);
  if (!surface?.allowedHosts.includes("app.page")) return false;
  return registry
    .availableNavigation()
    .some(
      (item) =>
        item.extensionId === target.extensionId &&
        item.targetSurfaceId === target.surfaceId &&
        PAGE_PLACEMENTS.includes(item.defaultPlacement),
    );
}

/** Navigation entries an extension may render as a top-level app page, i.e.
 * those whose target surface actually allows the `app.page` host. */
export function primaryNavigation(
  registry: ExtensionRegistry,
): NavigationContribution[] {
  const surfaces = registry.availableSurfaces();
  return registry
    .availableNavigation()
    .filter((item) => item.defaultPlacement === "mode.primary")
    .filter((item) =>
      surfaces.some(
        (surface) =>
          surface.extensionId === item.extensionId &&
          surface.id === item.targetSurfaceId &&
          surface.allowedHosts.includes("app.page"),
      ),
    );
}

export function activeSurface(
  registry: ExtensionRegistry,
  target: ExtensionTarget | null,
): SurfaceContribution | undefined {
  if (!target) return undefined;
  return registry
    .availableSurfaces()
    .find(
      (surface) =>
        surface.extensionId === target.extensionId &&
        surface.id === target.surfaceId,
    );
}

/** The surface behind an open extension page: available *and* still reachable
 * from a primary navigation entry, so the page always has an active control. */
export function activePage(
  registry: ExtensionRegistry,
  target: ExtensionTarget | null,
): SurfaceContribution | undefined {
  if (!target) return undefined;
  return pageReachable(registry, target)
    ? activeSurface(registry, target)
    : undefined;
}

export type ExtensionCommandTarget =
  | { kind: "page"; extensionId: string; surfaceId: string }
  | { kind: "pane"; extensionId: string; surfaceId: string }
  | { kind: "unavailable"; reason: string };

/** Commands are always addressed as (extensionId, commandId): a bare command
 * name would let one extension answer for another. */
export function resolveExtensionCommand(
  registry: ExtensionRegistry,
  snapshot: ExtensionSnapshot,
  extensionId: string,
  commandId: string,
): ExtensionCommandTarget {
  const command = snapshot.commands.find(
    (item) => item.extensionId === extensionId && item.id === commandId,
  );
  if (!command)
    return {
      kind: "unavailable",
      reason: `Extension command is unavailable: ${commandId}`,
    };
  const target = { extensionId, surfaceId: command.surfaceId };
  const surface = activeSurface(registry, target);
  if (!surface)
    return {
      kind: "unavailable",
      reason: `Extension surface is unavailable: ${command.surfaceId}`,
    };
  if (pageReachable(registry, target)) return { kind: "page", ...target };
  if (surface.allowedHosts.includes("workspace.pane"))
    return { kind: "pane", ...target };
  return {
    kind: "unavailable",
    reason: `Extension surface is unavailable: ${command.surfaceId}`,
  };
}

/** Navigation entries contributed to one placement, already ordered and
 * filtered down to those whose target surface is actually available. */
export function navigationFor(
  registry: ExtensionRegistry,
  placement: NavigationPlacement,
): NavigationContribution[] {
  const surfaces = registry.availableSurfaces();
  return registry
    .availableNavigation()
    .filter((item) => item.defaultPlacement === placement)
    .filter((item) =>
      surfaces.some(
        (surface) =>
          surface.extensionId === item.extensionId &&
          surface.id === item.targetSurfaceId,
      ),
    );
}

/** Workspace actions contributed to one placement. */
export function actionsFor(
  registry: ExtensionRegistry,
  placement: WorkspaceActionPlacement,
): WorkspaceActionContribution[] {
  return registry
    .availableActions()
    .filter((item) => item.defaultPlacement === placement);
}

/** Surfaces an extension asked to embed into one of the host pages. */
export function surfacesFor(
  registry: ExtensionRegistry,
  host: SurfaceHost,
): SurfaceContribution[] {
  return registry
    .availableSurfaces()
    .filter((surface) => surface.defaultHost === host);
}

export type NavigationTarget =
  | { kind: "page"; extensionId: string; surfaceId: string }
  | { kind: "pane"; extensionId: string; surfaceId: string }
  | { kind: "tab"; extensionId: string; surfaceId: string }
  | { kind: "unavailable"; reason: string };

/** Where a navigation entry leads: a full app page, a workspace pane, or a
 * pane that asked to be shown as a tab. The surface's own allowed hosts decide,
 * never the placement of the button that points at it. */
export function resolveNavigation(
  registry: ExtensionRegistry,
  item: { extensionId: string; targetSurfaceId: string },
): NavigationTarget {
  const target = {
    extensionId: item.extensionId,
    surfaceId: item.targetSurfaceId,
  };
  const surface = activeSurface(registry, target);
  if (!surface)
    return {
      kind: "unavailable",
      reason: `Extension surface is unavailable: ${item.targetSurfaceId}`,
    };
  if (surface.defaultHost === "app.page" && pageReachable(registry, target))
    return { kind: "page", ...target };
  if (surface.defaultHost === "workspace.tab")
    return { kind: "tab", ...target };
  if (surface.allowedHosts.includes("workspace.pane"))
    return { kind: "pane", ...target };
  if (pageReachable(registry, target)) return { kind: "page", ...target };
  return {
    kind: "unavailable",
    reason: `Extension surface is unavailable: ${item.targetSurfaceId}`,
  };
}

export type PaneOpen =
  | { kind: "show"; panel: Panel }
  | { kind: "add" }
  | { kind: "unavailable"; reason: string };

/** Whether opening a workspace surface should add a pane or reveal the one that
 * is already there. A surface that declared itself a singleton is never opened
 * twice, however it was reached - picker, toolbar action or command. */
export function resolvePaneOpen(
  registry: ExtensionRegistry,
  panels: Panel[],
  target: { extensionId: string; surfaceId: string },
): PaneOpen {
  const surface = activeSurface(registry, target);
  if (!surface)
    return {
      kind: "unavailable",
      reason: `Extension surface is unavailable: ${target.surfaceId}`,
    };
  const open = panels.find(
    (panel) =>
      panel.kind === "extension" &&
      panel.extension.extensionId === target.extensionId &&
      panel.extension.contributionId === target.surfaceId,
  );
  if (surface.instancePolicy === "singleton" && open)
    return { kind: "show", panel: open };
  return { kind: "add" };
}

/** A project is identified by where it lives, not just by its path: the same
 * path on a remote host is a different project.
 *
 * Only a remote endpoint changes the answer. A local Herdr socket is still
 * this machine, so folding it in would split a project from itself and break
 * again the moment the socket path moved. */
export function projectScope(cwd: string, connection?: string): string {
  if (!cwd) return "";
  return connection?.startsWith("ssh:") ? `${connection}\u0000${cwd}` : cwd;
}
