import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "./errors.ts";
import {
  applyDaemonEvent,
  applySessionList,
  daemonHost,
  emptyHost,
  reconcileSessions,
  type SessionsByHost,
} from "../daemonSessions.ts";
import type {
  ConnectionProfile,
  DaemonEvent,
  DaemonState,
  System,
  Workspace,
} from "../types";

export type DaemonConnection = "connected" | "offline" | "connecting";

const connectionOf = (state: DaemonState | undefined): DaemonConnection =>
  !state || state.state === "connecting"
    ? "connecting"
    : state.state === "ready"
      ? "connected"
      : "offline";

/** Binds the workspace list to the sessions the daemons run: one full
 * `session.list` per host when it becomes ready, then incremental events.
 * No polling. The workspace list itself lives in App; this hook only
 * reconciles known panels into it. */
export function useDaemon({
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
  const [states, setStates] = useState<Record<string, DaemonState>>({});
  const [errorByHost, setErrorByHost] = useState<Record<string, string>>({});
  const hosts = useRef<SessionsByHost>({});
  const generations = useRef<Record<string, number>>({});
  const listTokens = useRef<Record<string, number>>({});
  /** Events that arrive while a host's full list is in flight; the list is a
   * snapshot from before them, so they are replayed on top of it. */
  const buffered = useRef(new Map<string, DaemonEvent[]>());

  const reconcile = useCallback(
    () => setWorkspaces((items) => reconcileSessions(items, hosts.current)),
    [setWorkspaces],
  );

  const list = useCallback(
    async (host: string) => {
      if (!window.bridge) return;
      const token = (listTokens.current[host] ?? 0) + 1;
      listTokens.current[host] = token;
      buffered.current.set(host, []);
      try {
        const sessions = await window.bridge.sessionsList(host);
        if (listTokens.current[host] !== token) return;
        let entry = applySessionList(
          hosts.current[host] ?? emptyHost(),
          sessions,
        );
        for (const event of buffered.current.get(host) ?? [])
          entry = applyDaemonEvent(entry, event);
        buffered.current.delete(host);
        hosts.current[host] = entry;
        setErrorByHost((e) => (e[host] ? { ...e, [host]: "" } : e));
        reconcile();
      } catch (error) {
        if (listTokens.current[host] !== token) return;
        buffered.current.delete(host);
        setErrorByHost((e) => ({ ...e, [host]: errorText(error) }));
        throw error;
      }
    },
    [reconcile],
  );

  useEffect(() => {
    if (!window.bridge) {
      setErrorByHost((e) => ({
        ...e,
        local:
          "Browser preview. Start the desktop app for terminal and daemon access.",
      }));
      return;
    }
    const bridge = window.bridge;
    let stopped = false;
    bridge
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
    const onState = (state: DaemonState) => {
      const known = generations.current[state.host] ?? -1;
      if (state.generation < known) return;
      generations.current[state.host] = state.generation;
      setStates((s) => ({ ...s, [state.host]: state }));
      const entry = hosts.current[state.host] ?? emptyHost();
      if (state.state !== "ready") {
        // Keep what was known: a host that is not ready never ends a panel.
        hosts.current[state.host] = { ...entry, ready: false, listed: false };
        return;
      }
      if (entry.ready && state.generation === known) return;
      hosts.current[state.host] = { ...entry, ready: true, listed: false };
      void list(state.host).catch(() => {});
    };
    const onEvent = (event: DaemonEvent) => {
      if (event.generation < (generations.current[event.host] ?? -1)) return;
      if (event.method === "session.resync") {
        void list(event.host).catch(() => {});
        return;
      }
      buffered.current.get(event.host)?.push(event);
      const entry = hosts.current[event.host] ?? emptyHost();
      const next = applyDaemonEvent(entry, event);
      if (next === entry) return;
      hosts.current[event.host] = next;
      reconcile();
    };
    const offState = bridge.onDaemonState(onState);
    const offEvent = bridge.onDaemonEvent(onEvent);
    bridge
      .daemonStates()
      .then((all) => {
        if (!stopped) all.forEach(onState);
      })
      .catch((error) => {
        if (!stopped) notify(errorText(error));
      });
    return () => {
      stopped = true;
      offState();
      offEvent();
    };
  }, [list, notify, reconcile, setWorkspaces]);

  const refreshHerdr = useCallback(
    (endpoint: string) =>
      window.bridge && endpoint
        ? list(daemonHost(endpoint))
        : Promise.resolve(),
    [list],
  );
  // Sessions are pushed, so there is no cached snapshot to invalidate.
  const invalidateHerdr = useCallback((endpoint: string) => {
    void endpoint;
  }, []);

  const statusByEndpoint = useMemo(() => {
    const status: Record<string, DaemonConnection> = {};
    if (system?.socketPath)
      status[system.socketPath] = connectionOf(states.local);
    if (socket && !socket.startsWith("ssh:"))
      status[socket] = connectionOf(states.local);
    for (const profile of connectionProfiles)
      status[`ssh:${profile.id}`] = connectionOf(states[profile.id]);
    return status;
  }, [system, socket, states, connectionProfiles]);

  const socketHost = daemonHost(socket);
  const state = states[socketHost];
  return {
    system,
    socket,
    setSocket,
    connection: connectionOf(state),
    connectionError:
      errorByHost[socketHost] ||
      (state && state.state !== "ready" && state.state !== "connecting"
        ? state.message || state.reason || state.state
        : ""),
    refreshHerdr,
    invalidateHerdr,
    statusByEndpoint,
    /** The connection state of each host the manager reports. */
    daemonStates: states,
  };
}
