// Pure helpers the renderer bends the protocol into a screen with - no
// window.bridge access here, so they're cheap to unit test directly.
import type { Attempt, OrchestratorEvent, Task } from "./types";

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0s";
  const totalSeconds = Math.round(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours) return `${hours}h ${minutes}m`;
  if (minutes) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

export function formatCost(costUsd: number | undefined): string {
  return `$${(costUsd || 0).toFixed(2)}`;
}

export function attemptDurationMs(attempt: Attempt, now = Date.now()): number {
  return (attempt.endedAt ?? now) - attempt.startedAt;
}

export function totalDurationMs(task: Task, now = Date.now()): number {
  return task.attempts.reduce(
    (sum, attempt) => sum + attemptDurationMs(attempt, now),
    0,
  );
}

export function latestAttempt(task: Task): Attempt | undefined {
  return task.attempts[task.attempts.length - 1];
}

/** The latest attempt that actually implements the task, skipping the
 * planning attempt - an attempt count or a "criteria met" read should never
 * count a still-open plan as if it were real implementation progress. */
export function latestImplementAttempt(task: Task): Attempt | undefined {
  for (let i = task.attempts.length - 1; i >= 0; i--) {
    if (task.attempts[i].stage === "implement") return task.attempts[i];
  }
  return undefined;
}

/** The newest review-stage verdict, if any attempt has reached one. */
export function reviewOf(task: Task) {
  for (let i = task.attempts.length - 1; i >= 0; i--) {
    const attempt = task.attempts[i];
    if (attempt.stage === "review" && attempt.review) return attempt.review;
  }
  return undefined;
}

/** "2/5"-style attempt progress for a task's list row - `null` once the task
 * is no longer actively working (there is nothing to count toward). A queued
 * task that hasn't run yet counts its first attempt. */
export function attemptProgress(
  task: Task,
  maxAttempts?: number,
): string | null {
  if (task.status !== "running" && task.status !== "queued") return null;
  const n = latestImplementAttempt(task)?.n ?? 1;
  const max = maxAttempts ?? n;
  return `${n}/${max}`;
}

const STATUS_BADGE_LABELS: Record<Task["status"], string> = {
  drafting: "Drafting",
  queued: "Queued",
  running: "Running",
  waiting: "Waiting",
  done: "Done",
  stopped: "Stopped",
  failed: "Failed",
};

/** The short word a task-list pill badge shows - `statusDetail` carries
 * whatever else the row has to say underneath it. */
export function statusBadgeLabel(task: Task): string {
  return STATUS_BADGE_LABELS[task.status];
}

const REASON_MAX = 60;

function shortReason(task: Task): string {
  const detail =
    latestAttempt(task)?.failure?.detail ||
    task.decisions[task.decisions.length - 1] ||
    "";
  return detail.length > REASON_MAX
    ? `${detail.slice(0, REASON_MAX - 1)}…`
    : detail;
}

/** The task-list row's line under its pill: only what the pill and the
 * attempt progress don't already say, `""` when that is nothing. Never the
 * question text itself, so a row can't leak a private-looking question into
 * a list glanced at across a room. */
export function statusDetail(task: Task): string {
  switch (task.status) {
    case "running":
      return formatCost(task.costUsd);
    case "waiting":
      return "1 question for you";
    case "done": {
      const review = reviewOf(task);
      const cost = formatCost(task.costUsd);
      return review ? `review ${review.verdict} · ${cost}` : cost;
    }
    case "stopped":
    case "failed":
      return shortReason(task);
    default:
      return "";
  }
}

/** Whether a task's acceptance criteria can be shown as met: the task is
 * done, or its latest implement attempt already passed verify. */
export function criteriaMet(task: Task): boolean {
  return (
    task.status === "done" || latestImplementAttempt(task)?.status === "passed"
  );
}

/** Newest-updated first, matching `task.list`'s own order even after a live
 * `task` event patches one entry in place. */
export function upsertTask(tasks: Task[], task: Task): Task[] {
  const next = tasks.filter((item) => item.id !== task.id);
  next.push(task);
  next.sort((a, b) => b.updatedAt - a.updatedAt);
  return next;
}

export type OrchestratorLiveState = {
  tasks: Task[];
  /** Newest-last, capped, keyed by task id. */
  logLines: Record<string, string[]>;
};

export const emptyLiveState: OrchestratorLiveState = {
  tasks: [],
  logLines: {},
};

const MAX_LOG_LINES = 200;

/** Folds one `orchestrator-event` push into the panel's whole live state. */
export function applyOrchestratorEvent(
  state: OrchestratorLiveState,
  event: OrchestratorEvent,
): OrchestratorLiveState {
  if (event.event === "task")
    return { ...state, tasks: upsertTask(state.tasks, event.task) };
  if (event.event === "chat") return state;
  const lines = [...(state.logLines[event.taskId] || []), event.line].slice(
    -MAX_LOG_LINES,
  );
  return { ...state, logLines: { ...state.logLines, [event.taskId]: lines } };
}
