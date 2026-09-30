// Pure numbers behind the Analytics view and Home's one-line summary. No
// React, no daemon calls: tasks and cost summaries in, plain values out.
import type { SpendSummary, Task } from "./types";

export const DAY_MS = 24 * 60 * 60 * 1000;
const PERIOD_DAYS = 7;

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

export function isLanded(task: Task): boolean {
  return task.status === "done" && !!task.landedSha;
}

/** When the work reached its branch: the time the report was written. */
export function landedTime(task: Task): number | undefined {
  return task.reportAt;
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
  /** Every question asked: the answered ones in `questionHistory` plus the
   * ones open now. */
  questions: number;
  /** Answered by the answer policy, its judge or the orchestrator. */
  answeredForYou: number;
  overturned: number;
  /** Median ms from a question being asked to the owner's answer; null when
   * the owner answered none that carry an `askedAt`. */
  medianWaitMs: number | null;
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
  const history = inPeriod.flatMap((t) => t.questionHistory ?? []);
  const open = inPeriod.filter((t) => t.status === "waiting" && t.question);
  const waits = history.flatMap((q) =>
    q.answeredBy === "owner" && q.askedAt !== undefined
      ? [q.answeredAt - q.askedAt]
      : [],
  );
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
    questions: history.length + open.length,
    answeredForYou: history.filter((q) => q.answeredBy !== "owner").length,
    overturned: inPeriod.flatMap((t) =>
      (t.assumptions ?? []).filter(
        (a) => a.overturned && (a.by === "policy" || a.by === "judge"),
      ),
    ).length,
    medianWaitMs: median(waits),
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

/** The inclusive UTC `from`/`to` dates `costs.summary` windows a period
 * by: the first and last of its `dayKeys`. */
export function spendRange(period: Period): { from: string; to: string } {
  const keys = dayKeys(period);
  return { from: keys[0], to: keys[keys.length - 1] };
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

/** The Analytics insight about finished work that is not on its branch yet:
 * done tasks that only wait for a Land click, and `landing` tasks that wait
 * for their base checkout to be clean. Each part only when it applies. */
export function landInsight(ready: number, landing: Task[]): string {
  const tasks = (n: number) => `${n} finished task${n === 1 ? "" : "s"}`;
  return [
    ready ? `${tasks(ready)} ${ready === 1 ? "is" : "are"} ready to land` : "",
    landing.length
      ? `${tasks(landing.length)} ${landing.length === 1 ? "waits" : "wait"} for a clean checkout`
      : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** Top-level tasks orchd holds in `landing`: done and checked, waiting for a
 * clean checkout of their base branch. */
export function landingTasks(tasks: Task[]): Task[] {
  return tasks.filter(
    (t) => t.status === "landing" && !t.archived && !t.parent,
  );
}
