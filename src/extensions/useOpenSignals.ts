import { useEffect, useRef } from "react";
import type { Workspace } from "../types.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import { createOpenHandler } from "./openSignal.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** Opens the surface an agent asked for with `sushiai open` as the companion
 * half of its own pane (see `createOpenHandler`). Selection, focus and zoom
 * are never touched. */
export function useOpenSignals(
  workspaces: Workspace[],
  registry: ExtensionRegistry,
  ws: Pick<WorkspaceController, "openCompanion">,
) {
  const latest = useRef({ workspaces, registry, ws });
  latest.current = { workspaces, registry, ws };
  useEffect(() => {
    if (!window.bridge) return;
    return window.bridge.onDaemonEvent(
      createOpenHandler(
        () => latest.current,
        (panelId, target, args) =>
          latest.current.ws.openCompanion(panelId, target, args),
        (message) => console.warn(message),
      ),
    );
  }, []);
}
