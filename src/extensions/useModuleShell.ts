import { useCallback, useRef } from "react";
import { contains, uid } from "../layout.ts";
import type { Panel, Workspace } from "../types.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import { moduleUis, type ModuleShell } from "./modules.ts";
import type { ExtensionRegistry } from "./registry.ts";

type Where = Parameters<ModuleShell["openSurface"]>[0];

export type SurfacePlacement =
  | { kind: "reveal"; workspaceId: string; panel: Panel }
  | { kind: "add"; workspaceId: string }
  | { kind: "create"; cwd: string; connection?: string; name: string }
  | { kind: "missing" };

/** Where `openSurface` puts a surface: the first pane of it already in the
 * workspace's layout is revealed, otherwise one is added; a folder with no
 * workspace gets a workspace first. */
export function placeSurface(
  workspaces: Workspace[],
  where: Where,
  extensionId: string,
  surfaceId: string,
): SurfacePlacement {
  if ("cwd" in where) return { ...where, kind: "create" };
  const workspace = workspaces.find((item) => item.id === where.workspaceId);
  if (!workspace) return { kind: "missing" };
  const panel = workspace.panels.find(
    (item) =>
      item.kind === "extension" &&
      item.extension.extensionId === extensionId &&
      item.extension.contributionId === surfaceId &&
      contains(workspace.layout, item.id),
  );
  return panel
    ? { kind: "reveal", workspaceId: workspace.id, panel }
    : { kind: "add", workspaceId: workspace.id };
}

type Inputs = {
  ws: WorkspaceController;
  registry: ExtensionRegistry;
  showWorkspace(): void;
  switchWorkspace(id: string): void;
  notify(message: string): void;
};

/** Gives every module UI its shell hook. */
export function useModuleShell(inputs: Inputs) {
  const latest = useRef(inputs);
  latest.current = inputs;
  const openSurface = useCallback<ModuleShell["openSurface"]>(
    (where, extensionId, surfaceId, args) => {
      const { ws, registry, showWorkspace } = latest.current;
      const place = placeSurface(ws.workspaces, where, extensionId, surfaceId);
      if (place.kind === "missing") return;
      // Leave any section page or Agent/Chat mode first.
      showWorkspace();
      if (place.kind === "create") {
        void ws.createWorkspace(
          place.name,
          place.cwd,
          "shell",
          place.connection,
        );
        return;
      }
      if (place.kind === "reveal") {
        latest.current.switchWorkspace(place.workspaceId);
        ws.setSelected(place.panel.id);
        ws.setZoomed(place.panel.id);
        return;
      }
      try {
        const panel = registry.createPanel(extensionId, surfaceId, uid());
        if (Object.keys(args).length) panel.extension.args = { ...args };
        ws.insertPanel(panel, place.workspaceId);
      } catch (error) {
        latest.current.notify(
          error instanceof Error ? error.message : String(error),
        );
      }
    },
    [],
  );
  const shell = { workspaces: inputs.ws.workspaces, openSurface };
  // `moduleUis` is a constant list, so the hooks run in the same order on
  // every render.
  for (const module of moduleUis) {
    module.useShell(shell);
  }
}
