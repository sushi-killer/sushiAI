import { useExtensionRuntime } from "../extensions/runtime.ts";
import { ROUTINES_EXTENSION_ID } from "./id.ts";

/** Whether the Routines extension is switched on, read from the one
 * extensions snapshot: false until it has loaded. */
export function useRoutinesEnabled(): boolean {
  const { snapshot } = useExtensionRuntime();
  return snapshot.extensions.some(
    (extension) =>
      extension.manifest.id === ROUTINES_EXTENSION_ID &&
      extension.status === "active",
  );
}
