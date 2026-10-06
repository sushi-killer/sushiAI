import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "./errors.ts";
import {
  daemonHost,
  emptyFeed,
  emptyHost,
  failList,
  feedEvent,
  finishList,
  reconcileSessions,
  startList,
  type HostFeed,
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
  const feeds = useRef<Record<string, HostFeed>>({});

  const reconcile = useCallback(
    () => setWorkspaces((items) => reconcileSessions(items, hosts.current)),
    [setWorkspaces],
  );

  const list = useCallback(
    async (host: string) => {
      if (!window.bridge) return;
      const started = startList(feeds.current[host] ?? emptyFeed());
      feeds.current[host] = started.feed;
      try {
        const sessions = await window.bridge.sessionsList(host);
        const done = finishList(
          feeds.current[host],
          started.token,
          hosts.current[host] ?? emptyHost(),
          sessions,
        );
        if (!done) return;
        feeds.current[host] = done.feed;
        hosts.current[host] = done.host;
        setErrorByHost((e) => (e[host] ? { ...e, [host]: "" } : e));
        reconcile();
      } catch (error) {
        const feed = feeds.current[host];
        if (feed.token !== started.token) return;
        feeds.current[host] = failList(feed, started.token);
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
    // A failed full list is tried again with a growing pause while the host
    // stays ready; without it the panels of that host never reconcile.
    const retries = new Map<string, number>();
    const load = (host: string, pause = 1000) => {
      window.clearTimeout(retries.get(host));
      retries.delete(host);
      list(host).catch(() => {
        const entry = hosts.current[host];
        if (stopped || !entry?.ready || entry.listed) return;
        retries.set(
          host,
          window.setTimeout(
            () => load(host, Math.min(pause * 2, 15000)),
            pause,
          ),
        );
      });
    };
    const onState = (state: DaemonState) => {
      const feed = feeds.current[state.host] ?? emptyFeed();
      const known = feed.generation;
      if (state.generation < known) return;
      feeds.current[state.host] = { ...feed, generation: state.generation };
      setStates((s) => ({ ...s, [state.host]: state }));
      const entry = hosts.current[state.host] ?? emptyHost();
      if (state.state !== "ready") {
        // Keep what was known: a host that is not ready never ends a panel.
        hosts.current[state.host] = { ...entry, ready: false, listed: false };
        window.clearTimeout(retries.get(state.host));
        retries.delete(state.host);
        return;
      }
      if (entry.ready && state.generation === known) return;
      hosts.current[state.host] = { ...entry, ready: true, listed: false };
      load(state.host);
    };
    const onEvent = (event: DaemonEvent) => {
      const entry = hosts.current[event.host] ?? emptyHost();
      const step = feedEvent(
        feeds.current[event.host] ?? emptyFeed(),
        entry,
        event,
      );
      feeds.current[event.host] = step.feed;
      if (step.relist) load(event.host);
      if (step.host === entry) return;
      hosts.current[event.host] = step.host;
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
      retries.forEach((timer) => window.clearTimeout(timer));
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
    statusByEndpoint,
    /** The connection state of each host the manager reports. */
    daemonStates: states,
  };
}
