import { useCallback, useEffect, useRef } from "react";
import { LOCAL_ENDPOINT } from "../daemonSessions.ts";
import { contains, leaf, split, uid } from "../layout.ts";
import type { Panel, Workspace } from "../types.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import { moduleUis, type ModuleShell } from "./modules.ts";
import type { ExtensionRegistry } from "./registry.ts";
import { resolvePaneOpen } from "./routes.ts";

type Where = Parameters<ModuleShell["openSurface"]>[0];

export type SurfacePlacement =
  | { kind: "reveal"; workspaceId: string; panel: Panel }
  | { kind: "add"; workspaceId: string }
  | { kind: "create"; cwd: string; connection: string; name: string }
  | { kind: "unavailable"; reason: string }
  | { kind: "missing" };

/** Where `openSurface` puts a surface: the first pane of it already in the
 * workspace's layout is revealed, otherwise `resolvePaneOpen` decides (a
 * singleton's hidden pane is shown again, never doubled); a folder with no
 * workspace gets a workspace first, on the local host unless a connection is
 * named. */
export function placeSurface(
  workspaces: Workspace[],
  where: Where,
  registry: ExtensionRegistry,
  extensionId: string,
  surfaceId: string,
): SurfacePlacement {
  if ("cwd" in where)
    return {
      kind: "create",
      cwd: where.cwd,
      name: where.name,
      connection: where.connection ?? LOCAL_ENDPOINT,
    };
  const workspace = workspaces.find((item) => item.id === where.workspaceId);
  if (!workspace) return { kind: "missing" };
  const visible = workspace.panels.find(
    (item) =>
      item.kind === "extension" &&
      item.extension.extensionId === extensionId &&
      item.extension.contributionId === surfaceId &&
      contains(workspace.layout, item.id),
  );
  if (visible)
    return { kind: "reveal", workspaceId: workspace.id, panel: visible };
  const open = resolvePaneOpen(registry, workspace.panels, {
    extensionId,
    surfaceId,
  });
  if (open.kind === "unavailable") return open;
  return open.kind === "show"
    ? { kind: "reveal", workspaceId: workspace.id, panel: open.panel }
    : { kind: "add", workspaceId: workspace.id };
}

/** A surface waiting for the workspace of a folder that is being created. */
type Queued = {
  cwd: string;
  connection: string;
  extensionId: string;
  surfaceId: string;
  args: Record<string, string>;
};

/** The workspace a queued surface waits for, once it exists. */
export function queuedWorkspace(
  workspaces: Workspace[],
  queued: Pick<Queued, "cwd" | "connection">,
): Workspace | undefined {
  return workspaces.find(
    (item) =>
      item.cwd === queued.cwd &&
      (item.connection || LOCAL_ENDPOINT) === queued.connection,
  );
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
  const queued = useRef<Queued | null>(null);
  const openSurface = useCallback<ModuleShell["openSurface"]>(
    (where, extensionId, surfaceId, args) => {
      const { ws, registry, showWorkspace } = latest.current;
      const place = placeSurface(
        ws.workspaces,
        where,
        registry,
        extensionId,
        surfaceId,
      );
      if (place.kind === "missing") return;
      if (place.kind === "unavailable")
        return latest.current.notify(place.reason);
      // Leave any section page or Agent/Chat mode first.
      showWorkspace();
      if (place.kind === "create") {
        // The surface opens once the workspace exists; a failed launch drops it.
        const wait = { ...place, extensionId, surfaceId, args };
        queued.current = wait;
        void ws
          .createWorkspace(place.name, place.cwd, "shell", place.connection)
          .then((ok) => {
            if (!ok && queued.current === wait) queued.current = null;
          });
        return;
      }
      if (place.kind === "reveal") {
        const { panel, workspaceId } = place;
        latest.current.switchWorkspace(workspaceId);
        ws.updateWorkspace(workspaceId, (workspace) =>
          contains(workspace.layout, panel.id)
            ? workspace
            : {
                ...workspace,
                layout: workspace.layout
                  ? split(workspace.layout, leaf(panel.id), "row", 0.65)
                  : leaf(panel.id),
              },
        );
        ws.setSelected(panel.id);
        ws.setZoomed(panel.id);
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
  const { workspaces } = inputs.ws;
  useEffect(() => {
    const wait = queued.current;
    const workspace = wait && queuedWorkspace(workspaces, wait);
    if (!wait || !workspace) return;
    queued.current = null;
    openSurface(
      { workspaceId: workspace.id },
      wait.extensionId,
      wait.surfaceId,
      wait.args,
    );
  }, [workspaces, openSurface]);
  const shell = { workspaces, openSurface };
  // `moduleUis` is a constant list, so the hooks run in the same order on
  // every render.
  for (const module of moduleUis) {
    module.useShell(shell);
  }
}
