// Pure helpers for the Chat and Brainstorm views: session list grouping and
// times, and task references in a reply.
import type { Task } from "./types";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function startOfDay(ts: number): number {
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "now", "2m", "3h" today; a weekday within the week; else "Sep 3";
 * nothing for a session an older daemon sent without a time. */
export function sessionTime(ts: number | undefined, now = Date.now()): string {
  if (ts == null || !Number.isFinite(ts)) return "";
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

const timeOf = (session: { updatedAt?: number }): number | null =>
  session.updatedAt != null && Number.isFinite(session.updatedAt)
    ? session.updatedAt
    : null;

/** Newest first by orchd's `updatedAt`; a session without one sorts last. */
export function newestFirst(
  a: { updatedAt?: number },
  b: { updatedAt?: number },
): number {
  const [x, y] = [timeOf(a), timeOf(b)];
  if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
  return y - x;
}

/** Newest first, split into TODAY and EARLIER (a session without a time is
 * earlier). */
export function groupSessions<S extends { updatedAt?: number }>(
  sessions: S[],
  now = Date.now(),
): { label: "TODAY" | "EARLIER"; sessions: S[] }[] {
  const today = startOfDay(now);
  const newest = [...sessions].sort(newestFirst);
  const isToday = (s: S) => (timeOf(s) ?? -Infinity) >= today;
  return [
    {
      label: "TODAY" as const,
      sessions: newest.filter(isToday),
    },
    {
      label: "EARLIER" as const,
      sessions: newest.filter((s) => !isToday(s)),
    },
  ].filter((group) => group.sessions.length > 0);
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

/** A session row's preview: its last message with any text, on one line. */
export function lastMessageText(thread: {
  messages: { text: string }[];
}): string {
  const last = [...thread.messages].reverse().find((m) => m.text.trim());
  return last ? last.text.replace(/\s+/g, " ").trim() : "";
}

/** A session changed after the owner last looked at it. `since` stands in
 * for a session never opened here, so an old history never reads as new. */
export function isUnread(
  session: { updatedAt?: number },
  seenAt: number | undefined,
  since: number,
): boolean {
  const at = timeOf(session);
  return at !== null && at > (seenAt ?? since);
}
