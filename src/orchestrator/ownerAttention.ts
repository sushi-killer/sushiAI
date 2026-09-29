// Pure logic for which orchd tasks need the owner - what the Dock badge, the
// tray and the Inbox count. No React, no bridge.
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

/** One line in plain words for an Inbox row. */
export function ownerReason(task: Task): string {
  switch (task.status) {
    case "waiting":
      return questionsLabel(questionCount(task));
    case "landing":
      return "Waiting for a clean checkout to land";
    case "failed":
      return "Failed - needs your decision";
    default:
      return "Stopped on its own - needs a look";
  }
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
    focus: task.status === "waiting" ? "question" : "summary",
  };
}

export interface OwnerInboxRow {
  key: string;
  title: string;
  reason: string;
  open(): void;
}

/** The Inbox rows for the tasks that need the owner and match the search;
 * each row opens its task through `open`. */
export function ownerInboxRows(
  tasks: Task[],
  query: string,
  open: (target: TaskTarget) => void,
): OwnerInboxRow[] {
  return ownerTasks(tasks)
    .filter((task) => matchesOwnerTask(task, query))
    .map((task) => ({
      key: task.id,
      title: task.title,
      reason: ownerReason(task),
      open: () => open(ownerTarget(task)),
    }));
}
