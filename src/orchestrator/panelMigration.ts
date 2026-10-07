import type { Panel } from "../types.ts";

/** A panel saved before the orchestrator became a built-in extension pane:
 * `kind: "orchestrator"` with its view, host and repo on the panel. It loads
 * as the extension's pane with the same state as args. Data migration only;
 * the next save writes the new shape. Returns undefined for any other panel. */
export function migrateLegacyOrchestratorPanel(
  panel: Panel,
): Panel | undefined {
  const legacy = panel as unknown as Record<string, unknown>;
  if (legacy.kind !== "orchestrator") return undefined;
  const { orchestratorView, orchestratorHost, orchestratorRepo, ...rest } =
    legacy;
  const args: Record<string, string> = {};
  if (orchestratorView) args.view = JSON.stringify(orchestratorView);
  if (typeof orchestratorHost === "string") args.host = orchestratorHost;
  if (typeof orchestratorRepo === "string") args.repo = orchestratorRepo;
  return {
    ...rest,
    kind: "extension",
    extension: {
      extensionId: "builtin.orchestrator",
      contributionId: "orchestration",
      instanceId: String(legacy.id),
      stateVersion: 1,
      ...(Object.keys(args).length ? { args } : {}),
    },
  } as Panel;
}
