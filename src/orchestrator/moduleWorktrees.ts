import { useEffect, useMemo, useState } from "react";
import type { WorktreeClaim } from "../extensions/modules.ts";
import { orchestratorClient } from "./client.ts";
import { upsertTask } from "./helpers.ts";
import type { Task } from "./types.ts";

const MAX_LOAD_ATTEMPTS = 5;
const FINISHED = new Set(["done", "stopped", "failed"]);

/** Every orchd task across all repos, kept live, as worktree claims: the
 * title names the worktree and an unfinished task blocks its removal. Newest
 * first, an unfinished claim before a finished one. Any failure means no
 * claims. orchd runs on this Mac, so every claim is local. */
export function useOrchestratorWorktreeClaims(
  enabled: boolean,
): WorktreeClaim[] {
  const [tasks, setTasks] = useState<Task[]>([]);
  useEffect(() => {
    if (!enabled) {
      setTasks([]);
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // The daemon may still be starting when the app mounts, so retry a few
    // times with a growing delay before settling on no claims.
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
  return useMemo(() => claimsOf(tasks), [tasks]);
}

type ClaimTask = Pick<
  Task,
  "title" | "branch" | "worktree" | "repo" | "updatedAt" | "status"
>;

export function claimsOf(tasks: ClaimTask[]): WorktreeClaim[] {
  return tasks
    .map((task) => ({ task, active: !FINISHED.has(task.status) }))
    .sort(
      (a, b) =>
        Number(b.active) - Number(a.active) ||
        b.task.updatedAt - a.task.updatedAt,
    )
    .map(({ task, active }) => ({
      host: "local",
      repo: task.repo,
      path: task.worktree || undefined,
      branch: task.branch || undefined,
      label: task.title,
      active,
    }));
}
