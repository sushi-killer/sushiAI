import { useCallback, useEffect, useState } from "react";
import { errorText } from "./errors.ts";
import { useExtensionRuntime } from "../extensions/runtime.ts";

/** The extension registry plus its load state. `loaded` starts true outside the
 * desktop app, where there is no extension host to wait for. */
export function useExtensions(notify: (text: string) => void) {
  const { registry, snapshot } = useExtensionRuntime();
  const [loaded, setLoaded] = useState(!window.bridge?.extensionsList);

  useEffect(() => {
    let cancelled = false;
    if (!window.bridge?.extensionsList) return;
    window.bridge
      .extensionsList()
      .then((next) => {
        if (cancelled) return;
        registry.applySnapshot(next);
        setLoaded(true);
      })
      .catch((error) => {
        if (cancelled) return;
        setLoaded(true);
        notify(errorText(error));
      });
    return () => {
      cancelled = true;
    };
  }, [notify, registry]);

  const setEnabled = useCallback(
    async (extensionId: string, enabled: boolean) => {
      try {
        if (!window.bridge?.extensionsSetEnabled)
          return notify("Extensions can be changed from the desktop app.");
        registry.applySnapshot(
          await window.bridge.extensionsSetEnabled(extensionId, enabled),
        );
      } catch (error) {
        notify(errorText(error));
      }
    },
    [notify, registry],
  );

  const refresh = useCallback(async () => {
    try {
      if (!window.bridge?.extensionsRefresh)
        return notify("Extensions can be reloaded from the desktop app.");
      registry.applySnapshot(await window.bridge.extensionsRefresh());
    } catch (error) {
      notify(errorText(error));
    }
  }, [notify, registry]);

  return { registry, snapshot, loaded, setEnabled, refresh };
}
