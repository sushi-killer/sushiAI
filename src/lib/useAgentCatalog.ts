import { useEffect, useState } from "react";
import type { ChatModels } from "../types";

/** The agent catalog of a host. It is empty until that host's daemon is up, so
 * an empty answer never replaces a good one; it reloads when a daemon is ready. */
export function useAgentCatalog(endpoint?: string): ChatModels {
  const [catalog, setCatalog] = useState<ChatModels>({});
  useEffect(() => {
    let live = true;
    // Another host's catalog must not stand in while this one loads.
    setCatalog({});
    const load = () =>
      window.bridge
        ?.chatModels(endpoint)
        .then((value) => {
          if (live && Object.keys(value || {}).length) setCatalog(value);
        })
        .catch(() => {});
    load();
    const off = window.bridge?.onDaemonState?.((state) => {
      if (state.state === "ready") load();
    });
    return () => {
      live = false;
      off?.();
    };
  }, [endpoint]);
  return catalog;
}
