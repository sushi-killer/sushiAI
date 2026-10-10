// Pure logic behind the Log view: merging live entries, filtering by project,
// the "spent today" line and what a row opens. No React, no bridge.
import { formatCost } from "./helpers.ts";
import { projectName } from "./ownerAttention.ts";
import type { OverseerEntry } from "./types.ts";

/** Newest first, like `overseer.log`. */
export function addEntry(
  entries: OverseerEntry[],
  entry: OverseerEntry,
  limit = 2000,
): OverseerEntry[] {
  const same = (e: OverseerEntry) =>
    e.ts === entry.ts &&
    e.repo === entry.repo &&
    e.kind === entry.kind &&
    e.taskId === entry.taskId &&
    e.text === entry.text;
  if (entries.some(same)) return entries;
  return [entry, ...entries].sort((a, b) => b.ts - a.ts).slice(0, limit);
}

/** The projects that have entries, by name, for the filter. */
export function projectOptions(
  entries: OverseerEntry[],
): { repo: string; name: string }[] {
  const repos = [...new Set(entries.map((e) => e.repo))];
  return repos
    .map((repo) => ({ repo, name: projectName(repo) }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

export function filterEntries(
  entries: OverseerEntry[],
  repo: string | null,
): OverseerEntry[] {
  return repo ? entries.filter((e) => e.repo === repo) : entries;
}

/** "Spent today $1.20 of $5.00"; the budget part is hidden when it is 0. */
export function spentLine(spent: number, budget: number): string {
  return budget > 0
    ? `Spent today ${formatCost(spent)} of ${formatCost(budget)}`
    : `Spent today ${formatCost(spent)}`;
}

/** What a row opens: a digest or an owner note goes to the repo's chat; any
 * other row with a task goes to that task. */
export function rowTarget(entry: OverseerEntry): "chat" | "task" | null {
  if (entry.kind === "digest" || entry.kind === "asked_owner") return "chat";
  return entry.taskId ? "task" : null;
}

/** A row's text: one line, at most 200 chars (a hand-edited file may hold
 * more); a pull request reads "Opened PR #7" and keeps its URL for the title. */
export function displayText(entry: OverseerEntry): string {
  const number = /\/pull\/(\d+)/.exec(entry.text)?.[1];
  const text =
    entry.kind === "pr" && number ? `Opened PR #${number}` : entry.text;
  const line = text.split("\n")[0];
  return line.length > 200 ? `${line.slice(0, 199)}\u2026` : line;
}

/** "Today", "Yesterday" or "Oct 8" for a local day. */
export function dayLabel(ts: number, now = Date.now()): string {
  const day = new Date(ts);
  const today = new Date(now);
  const yesterday = new Date(now);
  yesterday.setDate(today.getDate() - 1);
  if (day.toDateString() === today.toDateString()) return "Today";
  if (day.toDateString() === yesterday.toDateString()) return "Yesterday";
  return day.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** Newest-first entries split into consecutive local days. */
export function groupByDay(
  entries: OverseerEntry[],
  now = Date.now(),
): { label: string; entries: OverseerEntry[] }[] {
  const groups: { label: string; day: string; entries: OverseerEntry[] }[] = [];
  for (const entry of entries) {
    const day = new Date(entry.ts).toDateString();
    const last = groups[groups.length - 1];
    if (last && last.day === day) last.entries.push(entry);
    else groups.push({ label: dayLabel(entry.ts, now), day, entries: [entry] });
  }
  return groups.map(({ label, entries }) => ({ label, entries }));
}

/** Today's UTC date, the window the daily budget gate counts. */
export function todayUtc(now = Date.now()): string {
  return new Date(now).toISOString().slice(0, 10);
}

/** The expanded panel's body: the full detail, else the row's own text. */
export function detailText(entry: OverseerEntry): string {
  return (entry.detail ?? entry.text).trim();
}

/** "Oct 8, 2026, 3:05 PM" for the expanded panel. */
export function fullDateTime(ts: number): string {
  return new Date(ts).toLocaleString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/** Which row is open after a click on `key`: one at a time, a second click closes. */
export function toggleOpen(open: string | null, key: string): string | null {
  return open === key ? null : key;
}

/** A stable key for a row, so the open state survives new entries on top. */
export function entryKey(entry: OverseerEntry): string {
  return `${entry.ts}|${entry.repo}|${entry.kind}|${entry.taskId ?? ""}`;
}
