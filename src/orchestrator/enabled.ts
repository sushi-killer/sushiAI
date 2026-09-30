import { useExtensionRuntime } from "../extensions/runtime.ts";

export const ORCHESTRATOR_EXTENSION_ID = "builtin.orchestrator";

/** Whether the orchestrator extension is switched on, read from the one
 * extensions snapshot. False until that snapshot has loaded, so nothing talks
 * to orchd before the app knows it should. */
export function useOrchestratorEnabled(): boolean {
  const { snapshot } = useExtensionRuntime();
  return snapshot.extensions.some(
    (extension) =>
      extension.manifest.id === ORCHESTRATOR_EXTENSION_ID &&
      extension.status === "active",
  );
}
