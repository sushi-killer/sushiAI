import { useEffect, useRef } from "react";
import type { Workspace } from "../types.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import {
  absoluteArg,
  detectOpenSignals,
  parseOpenSignal,
  resolveOpenSignal,
} from "./openSignal.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** Opens the surface an agent asked for as the companion half of its own pane
 * (see `openSignal.ts`). The text after the surface becomes `args.arg`.
 * Selection, focus and zoom are never touched. */
export function useOpenSignals(
  workspaces: Workspace[],
  registry: ExtensionRegistry,
  ws: Pick<WorkspaceController, "openCompanion">,
  notify: (message: string) => void,
) {
  const seen = useRef<Record<string, string>>({});
  useEffect(() => {
    const result = detectOpenSignals(seen.current, workspaces);
    seen.current = result.seen;
    for (const { workspace, value } of result.fresh) {
      const signal = parseOpenSignal(value);
      if (!signal) continue;
      const open = resolveOpenSignal(registry, workspace.panels, signal);
      if (open.kind === "ignored") continue;
      if (open.kind === "unavailable") notify(open.reason);
      else {
        const agent = workspace.panels.find((p) => p.id === open.panelId);
        ws.openCompanion(
          open.panelId,
          { extensionId: signal.extensionId, surfaceId: signal.surfaceId },
          { arg: absoluteArg(signal.arg, agent?.paneCwd || workspace.cwd) },
        );
      }
    }
  }, [workspaces, registry, ws, notify]);
}
