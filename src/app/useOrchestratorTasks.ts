import { useEffect, useState } from "react";
import { orchestratorClient } from "../orchestrator/client.ts";
import type { Task } from "../orchestrator/types.ts";

/** How often the list is re-read, so a daemon that was not up at launch
 * still shows up without waiting for a task event. */
const REFRESH_MS = 60000;

/** Every orchd task across repos, kept live from the daemon's task events.
 * Mounted once, by `useAttention`; the Inbox gets the list through
 * SectionPage's `attention` prop rather than subscribing again. */
export function useOrchestratorTasks(): Task[] {
  const [tasks, setTasks] = useState<Task[]>([]);
  useEffect(() => {
    if (!window.bridge) return;
    let cancelled = false;
    const load = () =>
      orchestratorClient
        .taskList()
        .then((list) => !cancelled && setTasks(list))
        .catch(() => {});
    void load();
    const timer = setInterval(load, REFRESH_MS);
    const off = window.bridge.onOrchestrator((event) => {
      if (event.event !== "task") return;
      const task = event.task;
      setTasks((old) =>
        old.some((t) => t.id === task.id)
          ? old.map((t) => (t.id === task.id ? task : t))
          : [...old, task],
      );
    });
    return () => {
      cancelled = true;
      clearInterval(timer);
      off?.();
    };
  }, []);
  return tasks;
}
