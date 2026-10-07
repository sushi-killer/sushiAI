// Pure text and rows behind Home: greeting, sub-line, the all-clear line,
// running rows and the "same failure" note. No React, no daemon calls.
import { attemptOf, formatDuration, latestAttempt } from "./helpers.ts";
import { questionSource } from "./taskDetailModel.ts";
import type { Task } from "./types.ts";

export function greeting(needYou: number): string {
  if (needYou === 0) return "Nothing needs you";
  return `${needYou} thing${needYou === 1 ? "" : "s"} need${needYou === 1 ? "s" : ""} you`;
}

export function activeCount(tasks: Task[]): number {
  return tasks.filter(
    (t) =>
      !t.archived &&
      (t.status === "running" ||
        t.status === "drafting" ||
        t.status === "queued" ||
        t.status === "waiting"),
  ).length;
}

/** "4 tasks active · $1.21 today · Orchestrator on Local"; the cost is left out
 * until it is known. On a remote host the line says where the orchestrator runs
 * instead (Figma "Home · remote host"). */
export function subLine(
  active: number,
  todayUsd: number | null,
  remoteHost?: string,
): string {
  if (remoteHost)
    return `Orchestrator on ${remoteHost} keeps running when your Mac sleeps`;
  return [
    `${active} task${active === 1 ? "" : "s"} active`,
    todayUsd === null ? "" : `$${todayUsd.toFixed(2)} today`,
    "Orchestrator on Local",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** "2 tasks running, 3 landed today." */
export function clearLine(running: number, landedToday: number): string {
  return `${running} task${running === 1 ? "" : "s"} running, ${landedToday} landed today.`;
}

/** "sushiai · attempt 1/4 · asked by verify · 2m ago" - who asked and when
 * only as far as orchd recorded it. */
export function questionMeta(
  repo: string,
  task: Task,
  maxAttempts?: number,
  now = Date.now(),
): string {
  return [
    repo,
    `attempt ${attemptOf(task, maxAttempts)}`,
    task.question ? questionSource(task.question, now) : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** " - the same test failed every attempt" when two or more implement
 * attempts failed with one signature, else "". */
export function sameFailureNote(task: Task): string {
  const failures = task.attempts
    .filter((a) => a.stage === "implement" && a.failure)
    .map((a) => a.failure!);
  if (failures.length < 2) return "";
  const first = failures[0];
  if (!failures.every((f) => f.signature === first.signature)) return "";
  return first.kind === "verify"
    ? " — the same test failed every attempt"
    : " — the same failure every attempt";
}

/** "implement · 6m", or "queued" when nothing is running yet. */
export function runningMeta(task: Task, now = Date.now()): string {
  const latest = latestAttempt(task);
  if (!latest || latest.status !== "running") {
    return task.status === "queued" ? "queued" : task.status;
  }
  return `${latest.stage} · ${formatDuration(now - latest.startedAt).split(" ")[0]}`;
}

/** The chat instruction "Run with a note" sends: the orchestrator agent, not
 * the UI, decides how the note reaches the task. */
export function runNoteMessage(task: Pick<Task, "id" | "title">, note: string) {
  return `Run task ${task.title} (${task.id}) again with this note from the owner: ${note.trim()}`;
}
