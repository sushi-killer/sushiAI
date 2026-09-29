// Pure helpers for the Chat and Brainstorm views: session list grouping and
// times, task references in a reply, and option chips a reply offers.
import type { Task } from "./types";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function startOfDay(ts: number): number {
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "now", "2m", "3h" today; a weekday within the week; else "Sep 3". */
export function sessionTime(ts: number, now = Date.now()): string {
  const age = now - ts;
  if (ts >= startOfDay(now)) {
    if (age < MINUTE) return "now";
    if (age < HOUR) return `${Math.floor(age / MINUTE)}m`;
    return `${Math.floor(age / HOUR)}h`;
  }
  if (age < 6 * DAY)
    return new Date(ts).toLocaleDateString("en-US", { weekday: "short" });
  return new Date(ts).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
}

export type SessionRow<S> = { session: S; time?: number; preview?: string };

/** Newest first, split into TODAY and EARLIER. A session with no known time
 * is new and empty when it has no title (TODAY), else older (EARLIER). */
export function groupSessions<S extends { id: string; title?: string }>(
  rows: SessionRow<S>[],
  now = Date.now(),
): { label: "TODAY" | "EARLIER"; rows: SessionRow<S>[] }[] {
  const today = startOfDay(now);
  const isToday = (row: SessionRow<S>) =>
    row.time !== undefined ? row.time >= today : !row.session.title;
  const newest = [...rows].reverse();
  return [
    { label: "TODAY" as const, rows: newest.filter(isToday) },
    { label: "EARLIER" as const, rows: newest.filter((r) => !isToday(r)) },
  ].filter((group) => group.rows.length > 0);
}

export function matchesQuery(
  query: string,
  ...fields: (string | undefined)[]
): boolean {
  const q = query.trim().toLowerCase();
  return !q || fields.some((f) => f?.toLowerCase().includes(q));
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Known tasks a reply names: by id (whole, or its first 8 characters) or by
 * a title of at least 12 characters, in the order they first appear. */
export function taskRefs(text: string, tasks: Task[], max = 3): Task[] {
  const lower = text.toLowerCase();
  const found: { task: Task; at: number }[] = [];
  for (const task of tasks) {
    const byId = new RegExp(
      `\\b${escapeRegExp(task.id.slice(0, 8).toLowerCase())}`,
    ).exec(lower);
    const title = task.title.trim().toLowerCase();
    const byTitle = title.length >= 12 ? lower.indexOf(title) : -1;
    const hits = [byId?.index ?? -1, byTitle].filter((i) => i >= 0);
    if (hits.length) found.push({ task, at: Math.min(...hits) });
  }
  return found
    .sort((a, b) => a.at - b.at)
    .slice(0, max)
    .map((f) => f.task);
}

const OPTIONS_FENCE = /```sushi-options[^\n]*\n([\s\S]*?)```/g;

/** Answers a reply offers in a ```sushi-options block (the last one wins). */
export function optionsInText(text: string): string[] {
  let options: string[] = [];
  for (const match of text.matchAll(OPTIONS_FENCE)) {
    try {
      const parsed: unknown = JSON.parse(match[1]);
      if (Array.isArray(parsed))
        options = parsed
          .filter((o): o is string => typeof o === "string" && !!o.trim())
          .map((o) => o.trim());
    } catch {
      // A half-written block offers nothing.
    }
  }
  return options;
}

export function stripOptions(text: string): string {
  return text.replace(OPTIONS_FENCE, "").trim();
}
