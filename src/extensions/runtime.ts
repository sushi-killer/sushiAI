import { useSyncExternalStore } from "react";
import { ExtensionRegistry } from "./registry.ts";

const registry = new ExtensionRegistry();

export function useExtensionRuntime() {
  const snapshot = useSyncExternalStore(
    (listener) => registry.subscribe(listener),
    () => registry.snapshot(),
    () => registry.snapshot(),
  );
  return { registry, snapshot };
}
