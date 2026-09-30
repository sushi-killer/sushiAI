import { useEffect, useState } from "react";
import { orchestratorClientFor } from "../orchestrator/client.ts";
import { upsertTask } from "../orchestrator/helpers.ts";
import { useOrchestratorEnabled } from "../orchestrator/enabled.ts";
import type { Task } from "../orchestrator/types.ts";

/** How often the list is re-read, so a daemon that was not up at launch
 * still shows up without waiting for a task event. */
const REFRESH_MS = 60000;

/** Every orchd task across repos, kept live from the daemon's task events;
 * empty, with no IPC at all, while the orchestrator is off.
 * Mounted once, by `useAttention`; the Inbox gets the list through
 * SectionPage's `attention` prop rather than subscribing again. */
export function useOrchestratorTasks(): Task[] {
  const enabled = useOrchestratorEnabled();
  const [tasks, setTasks] = useState<Task[]>([]);
  useEffect(() => {
    // Off: no polling, no subscription, and nothing left from when it was on.
    if (!window.bridge || !enabled) {
      setTasks([]);
      return;
    }
    let cancelled = false;
    // One list per host; a host that is down keeps what it last showed.
    const loaded = new Map<string, Task[]>();
    // Hosts whose list was actually read (not just given up on).
    const listed = new Set<string>();
    const publish = () => {
      if (!cancelled) setTasks([...loaded.values()].flat());
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
          publish();
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
      publish();
    };
    void load(true);
    const offHosts = window.bridge.onOrchestratorHosts(() => void load(false));
    const timer = setInterval(() => void load(true), REFRESH_MS);
    const off = window.bridge.onOrchestrator((event) => {
      if (event.event !== "task") return;
      const host = event.host || "local";
      loaded.set(host, upsertTask(loaded.get(host) ?? [], event.task));
      publish();
      // The daemon came up after launch (first use): read what it already has.
      if (!listed.has(host)) void loadHost(host);
    });
    return () => {
      cancelled = true;
      clearInterval(timer);
      offHosts();
      off?.();
    };
  }, [enabled]);
  return tasks;
}
