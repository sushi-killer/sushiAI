// Pure logic for which orchd tasks need the owner - what the Dock badge, the
// tray and the Inbox count. No React, no bridge.
import { plural } from "../lib/text.ts";
import type { TaskTarget } from "./notices.ts";
import type { Task } from "./types.ts";

/** How many questions a waiting task asks: a batched plan question says so in
 * its first line ("has 3 blocking question(s)"), anything else is one. */
export function questionCount(task: Task): number {
  const match = /has (\d+) blocking question/.exec(task.question?.text ?? "");
  const count = match ? Number(match[1]) : 1;
  return count >= 1 ? count : 1;
}

export function questionsLabel(count: number): string {
  return count === 1 ? "1 question for you" : `${count} questions for you`;
}

/** A stop the engine chose, not the owner: the owner's stop leaves an
 * "Owner: stop" decision or an interrupted attempt, a task parked for review
 * has no attempt at all. */
export function stoppedByEngine(task: Task): boolean {
  if (task.status !== "stopped") return false;
  const last = task.decisions[task.decisions.length - 1] ?? "";
  if (last.startsWith("Owner:")) return false;
  if (last.startsWith("Orchestrator:")) return true;
  const attempt = task.attempts[task.attempts.length - 1];
  return !!attempt && attempt.status !== "interrupted" && !!attempt.failure;
}

/** Waiting, failed, landing (dirty checkout) or stopped by the engine;
 * archived tasks never need anyone. */
export function needsOwner(task: Task): boolean {
  if (task.archived) return false;
  switch (task.status) {
    case "waiting":
    case "failed":
    case "landing":
      return true;
    case "stopped":
      return stoppedByEngine(task);
    default:
      return false;
  }
}

export function ownerTasks(tasks: Task[]): Task[] {
  return tasks
    .filter(needsOwner)
    .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

/** Inbox search: the title or the project the task belongs to. */
export function matchesOwnerTask(task: Task, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    task.title.toLowerCase().includes(needle) ||
    String(task.repo ?? "")
      .toLowerCase()
      .includes(needle)
  );
}

/** Where an Inbox row opens: the question for a waiting task, else the summary. */
export function ownerTarget(task: Task): TaskTarget {
  return {
    taskId: task.id,
    repo: task.repo,
    ...(task.host ? { host: task.host } : {}),
    focus: task.status === "waiting" ? "question" : "summary",
  };
}

/** The Inbox's task groups: a waiting task is a question to answer, anything
 * else that needs the owner is a decision. */
export type OwnerKind = "answer" | "decide";

export function ownerKind(task: Task): OwnerKind {
  return task.status === "waiting" ? "answer" : "decide";
}

/** The LAND group: finished top-level work whose branch is not on its base yet.
 * Deliberately separate from `needsOwner` - the Dock badge and the tray count
 * only what needs an answer or a decision, never a task that merely awaits
 * a Land click. */
export function landTasks(tasks: Task[]): Task[] {
  return tasks
    .filter(
      (task) =>
        task.status === "done" &&
        !task.archived &&
        !task.landedSha &&
        !task.parent &&
        !!task.baseRef,
    )
    .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

/** The last path segment of a repo: the project as the owner calls it. */
export function projectName(repo: string): string {
  return (
    repo
      .split(/[\\/]+/)
      .filter(Boolean)
      .pop() || repo
  );
}

/** The Inbox-zero line: what landed today and what is still running, each
 * only when there is something to say. */
export function inboxZeroSummary(tasks: Task[], now = Date.now()): string {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  const live = tasks.filter((task) => !task.archived);
  const landed = live.filter(
    (task) =>
      task.status === "done" &&
      !!task.landedSha &&
      task.updatedAt >= start.getTime(),
  ).length;
  const running = live.filter(
    (task) => task.status === "running" || task.status === "drafting",
  ).length;
  const parts: string[] = [];
  if (landed > 0) parts.push(`${plural(landed, "task")} landed today`);
  if (running > 0)
    parts.push(`${running} ${running === 1 ? "is" : "are"} still running`);
  return ["Nothing needs you.", parts.join("; ") && `${parts.join("; ")}.`]
    .filter(Boolean)
    .join(" ");
}
