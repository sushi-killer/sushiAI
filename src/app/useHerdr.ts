import { useCallback, useEffect, useMemo, useState } from "react";
import { errorText } from "./errors.ts";
import { reconcileHerdrWorkspaces } from "../herdrSnapshot.ts";
import type { ConnectionProfile, Snapshot, System, Workspace } from "../types";

export type HerdrConnection = "connected" | "offline" | "connecting";

/** The Herdr session: which socket is the app's current default, and a poll
 * per endpoint that keeps the workspace list in step with every host the
 * user is actually connected to - not just the default one. Several SSH
 * hosts (plus this Mac) can be live at the same time, each polled on its own
 * loop, so a workspace's panes never depend on whether its host happens to
 * be the one currently selected in Settings. */
export function useHerdr({
  savedSocket,
  notify,
  setWorkspaces,
  connectionProfiles,
}: {
  savedSocket: string;
  notify: (text: string) => void;
  setWorkspaces: React.Dispatch<React.SetStateAction<Workspace[]>>;
  connectionProfiles: ConnectionProfile[];
}) {
  const [system, setSystem] = useState<System | null>(null);
  const [socket, setSocket] = useState(savedSocket);
  const [statusByEndpoint, setStatusByEndpoint] = useState<
    Record<string, HerdrConnection>
  >({});
  const [errorByEndpoint, setErrorByEndpoint] = useState<
    Record<string, string>
  >({});

  useEffect(() => {
    if (!window.bridge) {
      setErrorByEndpoint((e) => ({
        ...e,
        [socket]:
          "Browser preview. Start the desktop app for terminal and Herdr access.",
      }));
      return;
    }
    window.bridge
      .system()
      .then((info) => {
        setSystem(info);
        setSocket((current) => current || info.socketPath);
        setWorkspaces((items) =>
          items.map((w) => ({ ...w, cwd: w.cwd || info.cwd })),
        );
      })
      .catch((error) => notify(errorText(error)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notify]);

  const home = system?.home || "";
  const refreshHerdr = useCallback(
    async (path: string) => {
      if (!window.bridge || !path) return;
      try {
        const response = await window.bridge.herdr(path, "session.snapshot");
        const snapshot: Snapshot = response.snapshot || response;
        if (
          !Array.isArray(snapshot.workspaces) ||
          !Array.isArray(snapshot.panes)
        )
          throw new Error("Unsupported Herdr snapshot response.");
        setWorkspaces((current) =>
          reconcileHerdrWorkspaces(current, snapshot, path, home),
        );
        setStatusByEndpoint((s) => ({ ...s, [path]: "connected" }));
        setErrorByEndpoint((e) => ({ ...e, [path]: "" }));
      } catch (error) {
        setStatusByEndpoint((s) => ({ ...s, [path]: "offline" }));
        setErrorByEndpoint((e) => ({ ...e, [path]: errorText(error) }));
      }
    },
    [home, setWorkspaces],
  );

  /** Every endpoint the app keeps live: this Mac, every SSH host with an open
   * tunnel, and whatever the current default socket is (a manually entered
   * one may not have a saved profile yet). */
  const endpoints = useMemo(() => {
    const list = new Set<string>();
    if (system?.socketPath) list.add(system.socketPath);
    for (const profile of connectionProfiles)
      if (profile.connected) list.add(`ssh:${profile.id}`);
    if (socket) list.add(socket);
    return [...list];
  }, [system, connectionProfiles, socket]);
  const endpointsKey = endpoints.join("|");

  useEffect(() => {
    if (!endpointsKey) return;
    let stopped = false;
    const stops = endpointsKey.split("|").map((endpoint) => {
      let timer: ReturnType<typeof setTimeout>;
      async function poll() {
        await refreshHerdr(endpoint);
        if (!stopped) timer = setTimeout(poll, 4000);
      }
      poll();
      return () => clearTimeout(timer);
    });
    return () => {
      stopped = true;
      stops.forEach((stop) => stop());
    };
  }, [endpointsKey, refreshHerdr]);

  const connection = statusByEndpoint[socket] ?? "connecting";
  const connectionError = errorByEndpoint[socket] ?? "";

  return {
    system,
    socket,
    setSocket,
    connection,
    connectionError,
    refreshHerdr,
    statusByEndpoint,
  };
}
