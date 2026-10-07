import { useCallback, useEffect, useMemo, useState } from "react";
import type { ConnectionProfile, DaemonState } from "../types";

/** Saved SSH connection profiles, shared by Settings (which edits them) and the
 * Sidebar (which only needs their names/live status to label workspace groups).
 * A profile is `connected` when the daemon on its host is ready; the app
 * connects autoConnect profiles itself. */
export function useConnectionProfiles() {
  const [saved, setSaved] = useState<ConnectionProfile[]>([]);
  const [ready, setReady] = useState<Record<string, boolean>>({});
  const refreshConnectionProfiles = useCallback(async () => {
    const profiles = await window.bridge?.connectionsList();
    if (profiles) setSaved(profiles);
  }, []);
  useEffect(() => {
    refreshConnectionProfiles();
  }, [refreshConnectionProfiles]);
  useEffect(() => {
    const bridge = window.bridge;
    if (!bridge) return;
    const apply = (state: DaemonState) =>
      setReady((current) =>
        current[state.host] === (state.state === "ready")
          ? current
          : { ...current, [state.host]: state.state === "ready" },
      );
    const off = bridge.onDaemonState(apply);
    void bridge.daemonStates().then((states) => states.forEach(apply));
    return off;
  }, []);
  const connectionProfiles = useMemo(
    () => saved.map((p) => ({ ...p, connected: Boolean(ready[p.id]) })),
    [saved, ready],
  );
  return { connectionProfiles, refreshConnectionProfiles };
}
