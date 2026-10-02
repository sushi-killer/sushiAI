import { useEffect, useRef } from "react";
import { uid } from "../layout.ts";
import type { Workspace } from "../types.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import {
  detectOpenSignals,
  parseOpenSignal,
  resolveOpenSignal,
} from "./openSignal.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** Opens the surface an agent asked for beside its own pane (see
 * `openSignal.ts`). The text after the surface becomes `args.arg`. Selection,
 * focus and zoom are never touched. */
export function useOpenSignals(
  workspaces: Workspace[],
  registry: ExtensionRegistry,
  ws: Pick<WorkspaceController, "openBeside" | "patchPanelArgs">,
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
      if (open.kind === "unavailable") {
        notify(open.reason);
      } else if (open.kind === "update") {
        ws.patchPanelArgs(open.panelId, { arg: signal.arg });
      } else {
        try {
          const panel = registry.createPanel(
            signal.extensionId,
            signal.surfaceId,
            uid(),
          );
          panel.extension.args = { arg: signal.arg };
          panel.extension.beside = open.besidePanelId;
          ws.openBeside(workspace.id, open.besidePanelId, panel);
        } catch (error) {
          notify(error instanceof Error ? error.message : String(error));
        }
      }
    }
  }, [workspaces, registry, ws, notify]);
}
