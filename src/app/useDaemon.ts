import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../lib/errors.ts";
import {
  LOCAL_ENDPOINT,
  emptyFeed,
  configureHibernation,
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
  notify,
  setWorkspaces,
  connectionProfiles,
}: {
  notify: (text: string) => void;
  setWorkspaces: React.Dispatch<React.SetStateAction<Workspace[]>>;
  connectionProfiles: ConnectionProfile[];
}) {
  const [system, setSystem] = useState<System | null>(null);
  const [states, setStates] = useState<Record<string, DaemonState>>({});
  const hosts = useRef<SessionsByHost>({});
  const [sessions, setSessions] = useState<SessionsByHost>({});
  const feeds = useRef<Record<string, HostFeed>>({});

  const reconcile = useCallback(() => {
    setSessions({ ...hosts.current });
    setWorkspaces((items) => reconcileSessions(items, hosts.current));
  }, [setWorkspaces]);

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
        reconcile();
      } catch (error) {
        const feed = feeds.current[host];
        if (feed.token !== started.token) return;
        feeds.current[host] = failList(feed, started.token);
        throw error;
      }
    },
    [reconcile],
  );

  useEffect(() => {
    if (!window.bridge) return;
    const bridge = window.bridge;
    let stopped = false;
    bridge
      .system()
      .then((info) => {
        if (stopped) return;
        setSystem(info);
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
        setSessions({ ...hosts.current });
        window.clearTimeout(retries.get(state.host));
        retries.delete(state.host);
        return;
      }
      if (entry.ready && state.generation === known) return;
      hosts.current[state.host] = { ...entry, ready: true, listed: false };
      // The idle-sleep delay is the app's setting; a host learns it on hello.
      bridge
        .appPreferences()
        .then((prefs) =>
          configureHibernation(bridge, state, prefs.hibernateAfterSecs),
        )
        .catch(() => {});
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

  const statusByEndpoint = useMemo(() => {
    const status: Record<string, DaemonConnection> = {
      [LOCAL_ENDPOINT]: connectionOf(states.local),
    };
    for (const profile of connectionProfiles)
      status[`ssh:${profile.id}`] = connectionOf(states[profile.id]);
    return status;
  }, [states, connectionProfiles]);

  return {
    system,
    /** This Mac's daemon. */
    connection: connectionOf(states.local),
    statusByEndpoint,
    /** The connection state of each host the manager reports. */
    daemonStates: states,
    /** The sessions each host runs, with their open asks (Inbox reads them). */
    sessions,
  };
}
