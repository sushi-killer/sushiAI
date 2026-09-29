// Pure numbers behind the Analytics view and Home's one-line summary. No
// React, no daemon calls: tasks and cost summaries in, plain values out.
import type { SpendSummary, Task } from "./types";

export const DAY_MS = 24 * 60 * 60 * 1000;
const PERIOD_DAYS = 7;

/** Optional fields the daemon may add; each is read defensively. */
export type TaskExtras = {
  /** When the task landed on its base branch, ms epoch. */
  landedAt?: number;
  /** Question history, once the daemon keeps it. */
  questions?: { askedAt?: number; answeredAt?: number; answeredBy?: string }[];
};

export type Period = { from: number; to: number };

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** The seven local days ending today, moved back `offset` whole periods. */
export function periodAt(now: number, offset = 0): Period {
  const to = startOfDay(now) + DAY_MS - offset * PERIOD_DAYS * DAY_MS;
  return { from: to - PERIOD_DAYS * DAY_MS, to };
}

export function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function extras(task: Task): TaskExtras {
  return task as Task & TaskExtras;
}

export function isLanded(task: Task): boolean {
  return task.status === "done" && !!task.landedSha;
}

/** When the work reached its branch: the daemon's `landedAt`, else the time
 * the report was written. */
export function landedTime(task: Task): number | undefined {
  return extras(task).landedAt ?? task.reportAt;
}

export function startedTime(task: Task): number {
  return task.attempts[0]?.startedAt ?? task.createdAt;
}

/** The tasks a period counts: top-level tasks created inside it that have
 * been started (a draft has not). */
export function cohort(tasks: Task[], period: Period): Task[] {
  return tasks.filter(
    (t) =>
      !t.parent &&
      t.status !== "drafting" &&
      t.createdAt >= period.from &&
      t.createdAt < period.to,
  );
}

export type PeriodStats = {
  started: number;
  landed: number;
  waitingToLand: number;
  failed: number;
  /** Failed tasks by the kind of their last failed attempt, biggest first. */
  failedBy: { kind: string; count: number }[];
  landedPercent: number | null;
  medianToLandMs: number | null;
  /** Questions the daemon kept: answered for the owner, open now, or in a
   * history it may provide. */
  questions: number;
  answeredForYou: number;
  overturned: number;
  /** Median ms from a question to its answer; null without history. */
  medianWaitMs: number | null;
  /** Whether `questions` came from a real history. */
  hasQuestionHistory: boolean;
};

function lastFailureKind(task: Task): string {
  for (let i = task.attempts.length - 1; i >= 0; i--) {
    const kind = task.attempts[i].failure?.kind;
    if (kind) return kind;
  }
  return "other";
}

export function periodStats(tasks: Task[], period: Period): PeriodStats {
  const inPeriod = cohort(tasks, period);
  const landed = inPeriod.filter(isLanded);
  const failed = inPeriod.filter((t) => t.status === "failed");
  const kinds = new Map<string, number>();
  for (const t of failed) {
    const kind = lastFailureKind(t);
    kinds.set(kind, (kinds.get(kind) ?? 0) + 1);
  }
  const toLand: number[] = [];
  for (const t of landed) {
    const at = landedTime(t);
    if (at !== undefined && at >= startedTime(t))
      toLand.push(at - startedTime(t));
  }
  const history = inPeriod.flatMap((t) => extras(t).questions ?? []);
  const hasQuestionHistory = history.length > 0;
  const assumptions = inPeriod.flatMap((t) =>
    (t.assumptions ?? []).filter((a) => a.by === "policy" || a.by === "judge"),
  );
  const open = inPeriod.filter((t) => t.status === "waiting" && t.question);
  const waits = history
    .filter((q) => q.askedAt !== undefined && q.answeredAt !== undefined)
    .map((q) => (q.answeredAt as number) - (q.askedAt as number));
  return {
    started: inPeriod.length,
    landed: landed.length,
    waitingToLand: inPeriod.filter((t) => t.status === "done" && !t.landedSha)
      .length,
    failed: failed.length,
    failedBy: [...kinds]
      .map(([kind, count]) => ({ kind, count }))
      .sort((a, b) => b.count - a.count || a.kind.localeCompare(b.kind)),
    landedPercent: inPeriod.length
      ? Math.round((landed.length / inPeriod.length) * 100)
      : null,
    medianToLandMs: median(toLand),
    questions: hasQuestionHistory
      ? history.length
      : assumptions.length + open.length,
    answeredForYou: assumptions.length,
    overturned: assumptions.filter((a) => a.overturned).length,
    medianWaitMs: median(waits),
    hasQuestionHistory,
  };
}

export type WeekSummary = {
  started: number;
  landed: number;
  waitingToLand: number;
  /** Null when no task started this week. */
  questionsPerTask: number | null;
  /** Null until the cost summary is known. */
  costUsd: number | null;
  /** "14 of 19 landed · 0.6 questions per task · $12.40" */
  text: string;
};

/** The last seven days in one line, for Home and the Analytics header. */
export function weekSummary(
  tasks: Task[],
  spend: Pick<SpendSummary, "totals"> | null | undefined,
  now = Date.now(),
): WeekSummary {
  const stats = periodStats(tasks, periodAt(now));
  const questionsPerTask = stats.started
    ? Math.round((stats.questions / stats.started) * 10) / 10
    : null;
  const costUsd = spend ? spend.totals.costUsd : null;
  const parts = [`${stats.landed} of ${stats.started} landed`];
  if (questionsPerTask !== null)
    parts.push(`${questionsPerTask} questions per task`);
  if (costUsd !== null) parts.push(`$${costUsd.toFixed(2)}`);
  return {
    started: stats.started,
    landed: stats.landed,
    waitingToLand: stats.waitingToLand,
    questionsPerTask,
    costUsd,
    text: parts.join(" · "),
  };
}

/** Seven `YYYY-MM-DD` keys, oldest first, as `costs.summary` groups a day
 * (UTC) - `to` is the exclusive end of the period. */
export function dayKeys(period: Period): string[] {
  return Array.from({ length: PERIOD_DAYS }, (_, i) =>
    new Date(period.from + i * DAY_MS + DAY_MS / 2).toISOString().slice(0, 10),
  );
}

export type DayBar = { key: string; label: string; costUsd: number };

/** One bar per day of the period; the last is "Today" when the period ends
 * today. */
export function spendBars(
  summary: Pick<SpendSummary, "rows">,
  period: Period,
  endsToday: boolean,
): DayBar[] {
  const byDay = new Map(summary.rows.map((r) => [r.key, r.costUsd]));
  const keys = dayKeys(period);
  return keys.map((key, i) => ({
    key,
    label:
      endsToday && i === keys.length - 1
        ? "Today"
        : new Date(`${key}T12:00:00Z`).toLocaleDateString("en-US", {
            weekday: "short",
            timeZone: "UTC",
          }),
    costUsd: byDay.get(key) ?? 0,
  }));
}

/** A signed delta as the strip prints it: "+4", "−$0.12". Null for no change. */
export function signed(
  value: number,
  format: (n: number) => string,
): string | null {
  if (!Number.isFinite(value) || value === 0) return null;
  return `${value > 0 ? "+" : "−"}${format(Math.abs(value))}`;
}
