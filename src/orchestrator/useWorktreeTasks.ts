import { useEffect, useMemo, useState } from "react";
import type { WorktreeTask } from "../app/workspaceMerge";
import { orchestratorClient } from "./client";
import { upsertTask } from "./helpers";
import { useOrchestratorEnabled } from "./enabled";
import type { Task } from "./types";

const MAX_LOAD_ATTEMPTS = 5;

/** Every orchd task across all repos, kept live, as the slice the sidebar
 * needs to name a task's worktree. Any failure means no tasks, which is
 * today's branch labels. */
export function useWorktreeTasks(): WorktreeTask[] {
  const enabled = useOrchestratorEnabled();
  const [tasks, setTasks] = useState<Task[]>([]);
  useEffect(() => {
    if (!enabled) {
      setTasks([]);
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // The daemon may still be starting when the app mounts, so retry a few
    // times with a growing delay before settling on branch labels.
    const load = (attempt: number) => {
      // A ping first: it never starts a daemon, so this is not a first use.
      orchestratorClient
        .probe()
        .then(() => orchestratorClient.taskList(undefined, true))
        .then((loaded) => {
          if (!cancelled) setTasks(loaded);
        })
        .catch(() => {
          if (!cancelled && attempt < MAX_LOAD_ATTEMPTS)
            timer = setTimeout(() => load(attempt + 1), 500 * 2 ** attempt);
        });
    };
    load(0);
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event === "task")
        setTasks((old) => upsertTask(old, event.task));
    });
    return () => {
      cancelled = true;
      clearTimeout(timer);
      off?.();
    };
  }, [enabled]);
  return useMemo(
    () =>
      tasks.map(({ title, branch, worktree, repo, updatedAt }) => ({
        title,
        branch,
        worktree,
        repo,
        updatedAt,
      })),
    [tasks],
  );
}
