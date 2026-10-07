import { useEffect, useSyncExternalStore } from "react";
import { orchestratorClientFor } from "./client.ts";
import { upsertTask } from "./helpers.ts";
import type { Task } from "./types.ts";

/** How often the list is re-read, so a daemon that was not up at launch
 * still shows up without waiting for a task event. */
const REFRESH_MS = 60000;

const NONE: Task[] = [];
let snapshot: Task[] = NONE;
const listeners = new Set<() => void>();
let users = 0;
let stop: (() => void) | null = null;

function publish(tasks: Task[]) {
  snapshot = tasks;
  for (const listener of [...listeners]) listener();
}

/** Reads every orchd task across repos and keeps it live from the daemon's
 * task events. Returns the function that stops all of it. */
export function startTaskStore(): () => void {
  if (!window.bridge) return () => {};
  let cancelled = false;
  // One list per host; a host that is down keeps what it last showed.
  const loaded = new Map<string, Task[]>();
  // Hosts whose list was actually read (not just given up on).
  const listed = new Set<string>();
  const push = () => {
    if (!cancelled) publish([...loaded.values()].flat());
  };
  // A ping first: it never starts a daemon, so this poll is not a first use.
  const loadHost = (host: string) => {
    const client = orchestratorClientFor(host);
    return client
      .probe()
      .then(() => client.taskList())
      .then((list) => {
        loaded.set(host, list);
        listed.add(host);
        push();
      })
      .catch(() => {
        // Counts as read, so only the timer retries a down host.
        if (!loaded.has(host)) loaded.set(host, []);
      });
  };
  const load = async (all: boolean) => {
    const hosts = await window.bridge!.orchestratorHosts().catch(() => []);
    const ids = new Set([
      "local",
      ...hosts.filter((h) => h.enabled).map((h) => h.id),
    ]);
    for (const host of [...loaded.keys()])
      if (!ids.has(host)) {
        loaded.delete(host);
        listed.delete(host);
      }
    // A host change only loads hosts not read yet: a host that is down
    // reports its error as a change, and re-reading it here would loop.
    await Promise.all(
      [...ids].filter((h) => all || !loaded.has(h)).map(loadHost),
    );
    push();
  };
  void load(true);
  const offHosts = window.bridge.onOrchestratorHosts(() => void load(false));
  // The module came up late (enabled, then the daemon restarted with the
  // `orch` capability) or the daemon reconnected: read the list now.
  const offDaemon = window.bridge.onDaemonState((state) => {
    if (state.state === "ready" && state.capabilities?.includes("orch"))
      void loadHost(state.host || "local");
  });
  const timer = setInterval(() => void load(true), REFRESH_MS);
  const off = window.bridge.onOrchestrator((event) => {
    if (event.event !== "task") return;
    const host = event.host || "local";
    loaded.set(host, upsertTask(loaded.get(host) ?? [], event.task));
    push();
    // The daemon came up after launch (first use): read what it already has.
    if (!listed.has(host)) void loadHost(host);
  });
  return () => {
    cancelled = true;
    clearInterval(timer);
    offHosts();
    offDaemon?.();
    off?.();
  };
}

function retain(): () => void {
  if (users++ === 0) stop = startTaskStore();
  return () => {
    if (--users > 0) return;
    stop?.();
    stop = null;
    // Nothing is left from when it was on.
    publish(NONE);
  };
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};
const read = () => snapshot;

/** The list as of now, for an action that runs outside a render. */
export const currentTasks = (): Task[] => snapshot;

/** Every orchd task across repos; empty, with no IPC at all, while the
 * orchestrator is off. One subscription is shared by every caller: the
 * Inbox rows (`moduleAttention`) and the Inbox detail read the same list. */
export function useTasks(enabled: boolean): Task[] {
  useEffect(() => (enabled ? retain() : undefined), [enabled]);
  const tasks = useSyncExternalStore(subscribe, read);
  return enabled ? tasks : NONE;
}
