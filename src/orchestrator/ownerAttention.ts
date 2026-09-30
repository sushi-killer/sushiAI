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

function pluralize(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** "2m", "14m", "1h", "3d": how long ago, for an attention item's corner. */
export function elapsedLabel(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}

/** "10 things need you across 4 projects on 2 hosts · oldest 1h". */
export function inboxHeadline(
  things: number,
  projects: number,
  hosts: number,
  oldestMs: number | null,
): string {
  if (things === 0) return "Nothing needs you";
  const verb = things === 1 ? "needs" : "need";
  const noun = things === 1 ? "thing" : "things";
  const base = `${things} ${noun} ${verb} you across ${pluralize(projects, "project", "projects")} on ${pluralize(hosts, "host", "hosts")}`;
  return oldestMs == null ? base : `${base} · oldest ${elapsedLabel(oldestMs)}`;
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
  if (landed > 0)
    parts.push(`${pluralize(landed, "task", "tasks")} landed today`);
  if (running > 0)
    parts.push(`${running} ${running === 1 ? "is" : "are"} still running`);
  return ["Nothing needs you.", parts.join("; ") && `${parts.join("; ")}.`]
    .filter(Boolean)
    .join(" ");
}

/** The reply field and a picked option become one answer, as in the task
 * view's question card: typed text replaces the pick, a pick alone is sent
 * as is. */
export function composeAnswer(pick: string, note: string): string {
  return note.trim() || pick;
}

/** A question's answer state. `pick` is the owner's own choice: undefined
 * until they pick, "" once they unpick. `preselected` is the first option,
 * shown picked up front so one click on Answer sends it. */
export type AnswerChoice = {
  pick: string | undefined;
  preselected: string;
  note: string;
};

/** The option a chip shows as picked: none while text is typed (it replaces
 * the pick), else the owner's pick, else the preselection. */
export function shownPick(choice: AnswerChoice): string {
  if (choice.note.trim()) return "";
  return choice.pick ?? choice.preselected;
}

/** Clicking the picked option unpicks it; any other one is picked. */
export function togglePick(choice: AnswerChoice, option: string): string {
  return shownPick(choice) === option ? "" : option;
}

/** What a click on Answer sends: the preselection counts, a click is
 * deliberate. */
export function clickAnswer(choice: AnswerChoice): string {
  return composeAnswer(choice.pick ?? choice.preselected, choice.note);
}

/** How long Enter is ignored after a question appears or the selection
 * moves to it, so a held or doubled Enter never answers the next one. */
export const ENTER_ARM_MS = 500;

/** What Enter sends, "" for nothing: only what the owner chose for this
 * question (an explicit pick or typed text), never the preselection, and
 * nothing within ENTER_ARM_MS of `shownAt`. */
export function enterAnswer(
  choice: AnswerChoice,
  shownAt: number,
  now: number,
): string {
  if (now - shownAt < ENTER_ARM_MS) return "";
  return composeAnswer(choice.pick ?? "", choice.note);
}

/** The key after `delta` steps from `current`, clamped to the list; the first
 * key when `current` is not in it. */
export function stepSelection(
  keys: string[],
  current: string | null,
  delta: number,
): string | null {
  if (keys.length === 0) return null;
  const at = current == null ? -1 : keys.indexOf(current);
  if (at < 0) return keys[0];
  return keys[Math.min(keys.length - 1, Math.max(0, at + delta))];
}
