import type { FailureRow, Proposal } from "./types";

/** A failure needs to repeat before it counts as recurring. */
export const MIN_RECURRENCE = 2;
export const TOP_FAILURES = 10;

const CLOSED: Proposal["status"][] = ["adopted", "rejected"];

/** Newest-first upsert by id, as the daemon's `proposal` events arrive. */
export function upsertProposal(rows: Proposal[], proposal: Proposal) {
  const index = rows.findIndex((row) => row.id === proposal.id);
  if (index < 0) return [proposal, ...rows];
  return rows.map((row, i) => (i === index ? proposal : row));
}

/** Proposals still waiting on the owner; the rail's Improvements count is the
 * same idea, so the section label matches it. */
export function openProposalCount(rows: Proposal[]) {
  return rows.filter((row) => !CLOSED.includes(row.status)).length;
}

/** "seen in 4 tasks", or nothing when the daemon gave no counts. */
export function seenIn(proposal: Proposal) {
  const count = proposal.before?.tasks;
  if (!count) return "";
  return `seen in ${count} ${count === 1 ? "task" : "tasks"}`;
}

/** The failures that keep coming back, most frequent first. */
export function recurringFailures(rows: FailureRow[]) {
  return rows
    .filter((row) => row.count >= MIN_RECURRENCE)
    .sort((a, b) => b.count - a.count)
    .slice(0, TOP_FAILURES);
}

/** The signature has its digits stripped for grouping; the example's first
 * line is a real one. */
export function failureSignature(row: FailureRow) {
  return row.exampleDetail.split("\n")[0].trim() || row.signature;
}

export function failureTitle(row: FailureRow) {
  return `${row.kind} · ${failureSignature(row)}`;
}

/** "in 3 tasks: Fix export, Sidebar titles, Owner attention". */
export function failureTasksLine(row: FailureRow) {
  const names = row.tasks.map((task) => task.title).join(", ");
  const count = row.tasks.length;
  return `in ${count} ${count === 1 ? "task" : "tasks"}${names ? `: ${names}` : ""}`;
}

/** What "Add a note" puts in the notes input for the owner to finish. */
export function failureNoteDraft(row: FailureRow) {
  return `Recurring ${row.kind} failure (${row.count}×): ${failureSignature(row)} - `;
}

/** "REPO NOTES · 3 - standing guidance for the planner". */
export function sectionLabel(label: string, count: number, hint?: string) {
  return `${label} · ${count}${hint ? ` — ${hint}` : ""}`;
}
