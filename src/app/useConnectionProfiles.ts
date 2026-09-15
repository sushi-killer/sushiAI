import { useCallback, useEffect, useRef, useState } from "react";
import type { ConnectionProfile } from "../types";

/** Saved SSH connection profiles, shared by Settings (which edits them) and the
 * Sidebar (which only needs their names/live status to label workspace groups). */
export function useConnectionProfiles() {
  const [connectionProfiles, setConnectionProfiles] = useState<
    ConnectionProfile[]
  >([]);
  const refreshConnectionProfiles = useCallback(async () => {
    const profiles = await window.bridge?.connectionsList();
    if (profiles) setConnectionProfiles(profiles);
  }, []);
  useEffect(() => {
    refreshConnectionProfiles();
  }, [refreshConnectionProfiles]);
  const autoConnected = useRef(false);
  useEffect(() => {
    if (autoConnected.current || !window.bridge || !connectionProfiles.length)
      return;
    autoConnected.current = true;
    const toConnect = connectionProfiles.filter(
      (p) => p.autoConnect && !p.connected,
    );
    if (!toConnect.length) return;
    Promise.all(
      toConnect.map((p) =>
        window.bridge!.connectionsConnect(`ssh:${p.id}`).catch(() => {}),
      ),
    ).then(refreshConnectionProfiles);
  }, [connectionProfiles, refreshConnectionProfiles]);
  return { connectionProfiles, refreshConnectionProfiles };
}
