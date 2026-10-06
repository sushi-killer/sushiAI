// The Inbox's items and filters, without React: orchd tasks and agent
// sessions in one queue, grouped by what the owner has to do with them.
import type { InboxGroup, InboxRow } from "./attention.ts";
import { groupKey } from "./workspaceMerge.ts";
import { formatCost } from "../orchestrator/helpers.ts";
import {
  elapsedLabel,
  enterAnswer,
  inboxHeadline,
  inboxZeroSummary,
  landTasks,
  matchesOwnerTask,
  ownerKind,
  projectName,
  type AnswerChoice,
} from "../orchestrator/ownerAttention.ts";
import type { Task } from "../orchestrator/types.ts";

export type Kind = "answer" | "decide" | "land" | "panels" | "working" | "idle";

type Base = {
  key: string;
  title: string;
  project: string;
  /** Local key of the host the item lives on. */
  host: string;
  at: number | null;
};
export type TaskItem = Base & {
  source: "task";
  kind: "answer" | "decide" | "land";
  task: Task;
};
export type SessionItem = Base & {
  source: "session";
  kind: "answer" | "panels" | "working" | "idle";
  row: InboxRow;
};
export type Item = TaskItem | SessionItem;

/** Display order; the first four are what "needs you" counts. */
export const KINDS: Kind[] = [
  "answer",
  "decide",
  "land",
  "panels",
  "working",
  "idle",
];
const NEEDS: Kind[] = ["answer", "decide", "land", "panels"];
export const needsYou = (item: Item) => NEEDS.includes(item.kind);

export const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini",
  "cursor-agent": "Cursor",
};

export function agentName(row: InboxRow): string {
  return (row.panel.agent && AGENT_NAMES[row.panel.agent]) || row.panel.title;
}

const SESSION_KIND = {
  blocked: "answer",
  done: "panels",
  working: "working",
  idle: "idle",
} as const;

/** Every item, in group order and newest first within a group. Shells are
 * left out: they only ever appear in the cleanup review. */
export function inboxItems(
  ownerTasks: Task[],
  allTasks: Task[],
  groups: InboxGroup[],
  orchestrator = true,
  askPanels: ReadonlySet<string> = new Set(),
): Item[] {
  const out: Item[] = [];
  const task = (kind: TaskItem["kind"], t: Task, prefix: string) =>
    out.push({
      source: "task",
      key: `${prefix}:${t.id}`,
      kind,
      title: t.title,
      project: projectName(t.repo),
      host: groupKey(t.host),
      at: t.updatedAt,
      task: t,
    });
  if (orchestrator) {
    for (const t of ownerTasks) task(ownerKind(t), t, "task");
    for (const t of landTasks(allTasks)) task("land", t, "land");
  }
  for (const group of groups) {
    if (group.key === "shells") continue;
    for (const row of group.rows)
      out.push({
        source: "session",
        key: `panel:${row.panel.id}`,
        // A panel with an open ask needs the owner whatever its status says.
        kind: askPanels.has(row.panel.id) ? "answer" : SESSION_KIND[group.key],
        title: `${row.workspace.name} · ${agentName(row)}`,
        project: row.workspace.name,
        host: groupKey(row.workspace.connection),
        at: row.since,
        row,
      });
  }
  const rank = (kind: Kind) => KINDS.indexOf(kind);
  return out.sort(
    (a, b) => rank(a.kind) - rank(b.kind) || (b.at ?? 0) - (a.at ?? 0),
  );
}

export type Scope = { project: string; host: string; query: string };

export function inScope(item: Item, { project, host, query }: Scope): boolean {
  if (project && item.project !== project) return false;
  if (host && item.host !== host) return false;
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return item.source === "task"
    ? matchesOwnerTask(item.task, needle)
    : `${item.row.panel.title} ${item.title}`.toLowerCase().includes(needle);
}

/** What "Land N" lands: exactly the LAND rows on screen, never the ones a
 * filter hides. */
export function landTargets(visible: Item[]): Task[] {
  return visible.flatMap((item) =>
    item.source === "task" && item.kind === "land" ? [item.task] : [],
  );
}

/** What "Clean up" offers for review within the current host and project
 * filters: idle agents (ticked by default) and plain shells (listed apart and
 * never ticked by default - a shell may be running a dev server). */
export function cleanupCandidates(
  groups: InboxGroup[],
  { project, host }: Pick<Scope, "project" | "host">,
): { agents: InboxRow[]; shells: InboxRow[] } {
  const rows = (key: "idle" | "shells") =>
    (groups.find((group) => group.key === key)?.rows ?? []).filter(
      (row) =>
        (!host || groupKey(row.workspace.connection) === host) &&
        (!project || row.workspace.name === project),
    );
  return { agents: rows("idle"), shells: rows("shells") };
}

/** "1 idle session", "2 idle sessions": a count and its noun, whose last
 * word takes an "s" unless the count is one. */
export function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/** "<1m" under a minute, else "2m", "1h", "3d". */
export function ageLabel(ms: number): string {
  return ms < 60_000 ? "<1m" : elapsedLabel(ms);
}

/** The subtitle for the items in view: "3 things need you across 1 project
 * on 1 host · oldest 2m", counted within the project, host and search
 * filters. */
export function scopedHeadline(
  needs: Item[],
  filtered: boolean,
  now: number,
): string {
  if (needs.length === 0)
    return filtered ? "Nothing here needs you" : "Nothing needs you";
  const oldest = Math.min(
    ...needs.map((item) => (item.at == null ? now : item.at)),
  );
  const base = inboxHeadline(
    needs.length,
    new Set(needs.map((item) => item.project)).size,
    new Set(needs.map((item) => item.host)).size,
    null,
  );
  return needs.some((item) => item.at != null)
    ? `${base} · oldest ${ageLabel(now - oldest)}`
    : base;
}

/** What Enter sends in the Inbox. Enter stays unarmed for a moment after the
 * selection moves (see `enterAnswer`), except after a digit or click pick
 * made while this question was shown: that pick is as deliberate as a click
 * on Answer, so the Enter right after it sends it. */
export function inboxEnterAnswer(
  choice: AnswerChoice,
  selectedAt: number,
  pickedAt: number | undefined,
  now: number,
): string {
  const picked = pickedAt != null && pickedAt >= selectedAt;
  return enterAnswer(choice, picked ? -Infinity : selectedAt, now);
}

/** The detail's branch box after the branch and +/−: "3 files · attempt 1/4
 * · $0.04", each part only when there is something to say. */
export function diffFacts(
  files: number | undefined,
  attempts: number,
  maxAttempts: number | undefined,
  costUsd: number | undefined,
): string {
  return [
    files ? plural(files, "file") : "",
    attempts > 0
      ? `attempt ${attempts}${maxAttempts ? `/${maxAttempts}` : ""}`
      : "",
    costUsd && costUsd > 0 ? formatCost(costUsd) : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The Inbox-zero line, which always says how the day went: "Nothing needs
 * you. Nothing landed today yet." when nothing landed or runs. */
export function zeroLine(
  tasks: Task[],
  now = Date.now(),
  orchestrator = true,
): string {
  if (!orchestrator) return "Nothing needs you.";
  const summary = inboxZeroSummary(tasks, now);
  return summary === "Nothing needs you."
    ? `${summary} Nothing landed today yet.`
    : summary;
}
