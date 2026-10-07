import { useMemo } from "react";
import type { WorktreeClaim } from "../extensions/modules.ts";
import { useTasks } from "./useTasks.ts";
import type { Task } from "./types.ts";

const FINISHED = new Set(["done", "stopped", "failed"]);

/** Every task across all repos and hosts, from the one shared store, as
 * worktree claims: the title names the worktree and an unfinished task blocks
 * its removal. Newest first, an unfinished claim before a finished one. */
export function useOrchestratorWorktreeClaims(
  enabled: boolean,
): WorktreeClaim[] {
  const tasks = useTasks(enabled);
  return useMemo(() => claimsOf(tasks), [tasks]);
}

type ClaimTask = Pick<
  Task,
  "title" | "branch" | "worktree" | "repo" | "updatedAt" | "status" | "host"
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
      host: task.host || "local",
      repo: task.repo,
      path: task.worktree || undefined,
      branch: task.branch || undefined,
      label: task.title,
      active,
    }));
}
