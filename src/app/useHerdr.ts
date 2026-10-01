import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "./errors.ts";
import { reconcileHerdrWorkspaces } from "../herdrSnapshot.ts";
import { HERDR_RECONCILE_MS, SnapshotCoordinator } from "../herdrSync.ts";
import type { ConnectionProfile, Snapshot, System, Workspace } from "../types";

export type HerdrConnection = "connected" | "offline" | "connecting";

export function useHerdr({
  savedSocket,
  notify,
  workspaces,
  setWorkspaces,
  connectionProfiles,
}: {
  savedSocket: string;
  notify: (text: string) => void;
  workspaces: Workspace[];
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
  const coordinators = useRef(new Map<string, SnapshotCoordinator<Snapshot>>());
  const callbacks = useRef({ setWorkspaces, home: system?.home || "" });
  callbacks.current = { setWorkspaces, home: system?.home || "" };

  useEffect(() => {
    if (!window.bridge) {
      setErrorByEndpoint((e) => ({
        ...e,
        [savedSocket]:
          "Browser preview. Start the desktop app for terminal and Herdr access.",
      }));
      return;
    }
    let stopped = false;
    window.bridge
      .system()
      .then((info) => {
        if (stopped) return;
        setSystem(info);
        setSocket((current) => current || info.socketPath);
        setWorkspaces((items) =>
          items.map((w) => ({ ...w, cwd: w.cwd || info.cwd })),
        );
      })
      .catch((error) => {
        if (!stopped) notify(errorText(error));
      });
    return () => {
      stopped = true;
    };
  }, [notify, savedSocket, setWorkspaces]);

  const coordinator = useCallback((endpoint: string) => {
    let entry = coordinators.current.get(endpoint);
    if (!entry) {
      entry = new SnapshotCoordinator(
        async () => {
          const response = await window.bridge!.herdr(
            endpoint,
            "session.snapshot",
          );
          const snapshot: Snapshot = response.snapshot || response;
          if (
            !Array.isArray(snapshot.workspaces) ||
            !Array.isArray(snapshot.panes)
          )
            throw new Error("Unsupported Herdr snapshot response.");
          return snapshot;
        },
        (snapshot) => {
          callbacks.current.setWorkspaces((current) =>
            reconcileHerdrWorkspaces(
              current,
              snapshot,
              endpoint,
              callbacks.current.home,
            ),
          );
          setStatusByEndpoint((s) => ({ ...s, [endpoint]: "connected" }));
          setErrorByEndpoint((e) => ({ ...e, [endpoint]: "" }));
        },
        (error) => {
          setStatusByEndpoint((s) => ({ ...s, [endpoint]: "offline" }));
          setErrorByEndpoint((e) => ({ ...e, [endpoint]: errorText(error) }));
        },
      );
      coordinators.current.set(endpoint, entry);
    }
    return entry;
  }, []);

  const refreshHerdr = useCallback(
    (path: string) =>
      window.bridge && path ? coordinator(path).refresh() : Promise.resolve(),
    [coordinator],
  );
  const invalidateHerdr = useCallback(
    (path: string) => coordinator(path).invalidate(),
    [coordinator],
  );
  const endpoints = new Set<string>();
  if (system?.socketPath) endpoints.add(system.socketPath);
  if (socket) endpoints.add(socket);
  for (const workspace of workspaces)
    if (
      workspace.herdrId &&
      workspace.connection &&
      !workspace.connection.startsWith("ssh:")
    )
      endpoints.add(workspace.connection);
  for (const profile of connectionProfiles)
    if (profile.connected || `ssh:${profile.id}` === socket)
      endpoints.add(`ssh:${profile.id}`);
  const endpointsKey = JSON.stringify([...endpoints].sort());

  useEffect(() => {
    if (!window.bridge) return;
    const wanted: string[] = JSON.parse(endpointsKey);
    const generations = new Map<string, number>();
    const subscriptionId = crypto.randomUUID();
    const unsubscribe = window.bridge.onHerdr((event) => {
      if (!wanted.includes(event.endpoint)) return;
      const previous = generations.get(event.endpoint) ?? -1;
      if (event.generation < previous) return;
      const entry = coordinator(event.endpoint);
      if (event.generation > previous) entry.invalidate();
      generations.set(event.endpoint, event.generation);
      if (event.type === "disconnected") {
        entry.disconnect();
        setStatusByEndpoint((s) => ({ ...s, [event.endpoint]: "offline" }));
        if (event.error)
          setErrorByEndpoint((e) => ({
            ...e,
            [event.endpoint]: event.error!.message,
          }));
      } else if (event.type === "connected") {
        void entry.reconnect();
      } else {
        entry.invalidate();
        void entry.refresh();
      }
    });
    for (const endpoint of wanted) {
      void window.bridge
        .herdrSubscribe(endpoint, subscriptionId)
        .catch((error) => {
          setStatusByEndpoint((s) => ({ ...s, [endpoint]: "offline" }));
          setErrorByEndpoint((e) => ({ ...e, [endpoint]: errorText(error) }));
        });
      void refreshHerdr(endpoint);
    }
    const timer = setInterval(
      () => wanted.forEach((endpoint) => void refreshHerdr(endpoint)),
      HERDR_RECONCILE_MS,
    );
    const entries = coordinators.current;
    return () => {
      clearInterval(timer);
      unsubscribe();
      for (const endpoint of wanted) {
        window.bridge
          ?.herdrUnsubscribe(endpoint, subscriptionId)
          .catch(() => {});
        entries.get(endpoint)?.close();
        entries.delete(endpoint);
      }
    };
  }, [endpointsKey, coordinator, refreshHerdr]);

  return {
    system,
    socket,
    setSocket,
    connection: statusByEndpoint[socket] ?? "connecting",
    connectionError: errorByEndpoint[socket] ?? "",
    refreshHerdr,
    invalidateHerdr,
    statusByEndpoint,
  };
}
