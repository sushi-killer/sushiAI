import { useEffect, useMemo, useState } from "react";
import { ArrowRight, ChevronLeft, ChevronRight, GitMerge } from "lucide-react";
import { useOrchestratorClient } from "./hostContext";
import { errorText, formatCost } from "./helpers";
import { landTasks } from "./ownerAttention";
import {
  cohort,
  periodAt,
  periodStats,
  signed,
  spendBars,
  type Period,
  type PeriodStats,
} from "./stats";
import type { SpendGroup, SpendSummary, Task } from "./types";
import "./analytics.css";

/** A cost summary that echoes the range it was asked for, once orchd
 * supports `from`/`to`. */
type RangedSummary = SpendSummary & { from?: number; to?: number };

type Spend = {
  day: RangedSummary;
  stage: RangedSummary;
  task: RangedSummary;
  /** The daemon honoured `from`/`to`: previous periods can be asked for. */
  ranged: boolean;
};

type Tone = "ok" | "warning";
type Note = { text: string; tone?: Tone };

function shortDuration(ms: number): string {
  const minutes = Math.round(ms / 60000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const rest = minutes % 60;
  return rest ? `${Math.floor(minutes / 60)}h ${rest}m` : `${minutes / 60}h`;
}

function dayLabel(ms: number): string {
  return new Date(ms).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
}

function periodRange(period: Period): string {
  const first = new Date(period.from);
  const last = new Date(period.to - 1);
  const end =
    first.getMonth() === last.getMonth()
      ? String(last.getDate())
      : dayLabel(last.getTime());
  return `${dayLabel(period.from)}–${end}`;
}

async function loadSpend(
  orchestratorClient: ReturnType<
    typeof import("./client").orchestratorClientFor
  >,
  cwd: string,
  period: Period,
  offset: number,
): Promise<Spend> {
  const query = (groupBy: SpendGroup) => {
    // Sent with `sinceDays` too: a daemon that ignores the range answers for
    // the last week, which is the period being shown until the pager exists.
    const params = {
      repo: cwd,
      sinceDays: 7,
      from: period.from,
      to: period.to,
      groupBy: [groupBy],
    };
    return orchestratorClient.costsSummary(params) as Promise<RangedSummary>;
  };
  const [day, stage, task] = await Promise.all([
    query("day"),
    query("stage"),
    query("task"),
  ]);
  const ranged = day.from !== undefined || offset > 0;
  return { day, stage, task, ranged };
}

async function loadPrevious(
  orchestratorClient: ReturnType<
    typeof import("./client").orchestratorClientFor
  >,
  cwd: string,
  period: Period,
): Promise<RangedSummary | null> {
  const previous = periodAt(period.to - 1, 1);
  const params = {
    repo: cwd,
    from: previous.from,
    to: previous.to,
    groupBy: ["day"] as SpendGroup[],
  };
  try {
    const summary = (await orchestratorClient.costsSummary(
      params,
    )) as RangedSummary;
    return summary.from === undefined ? null : summary;
  } catch {
    return null;
  }
}

/** Lower is better for every delta on this page except "landed". */
function delta(
  value: number,
  format: (n: number) => string,
  higherIsBetter = false,
): Note | undefined {
  const text = signed(value, format);
  if (!text) return undefined;
  const good = higherIsBetter ? value > 0 : value < 0;
  return { text, tone: good ? "ok" : "warning" };
}

function Cell({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note?: Note;
}) {
  return (
    <div className="an-cell">
      <span className="an-cell-label">{label}</span>
      <span className="an-cell-value">
        <span className="an-cell-number">{value}</span>
        {note && (
          <span className={`an-note${note.tone ? ` ${note.tone}` : ""}`}>
            {note.text}
          </span>
        )}
      </span>
    </div>
  );
}

type RowData = {
  label: string;
  value: string;
  note?: Note;
  onOpen?: () => void;
};

function Rows({ title, rows }: { title: string; rows: RowData[] }) {
  if (rows.length === 0) return null;
  return (
    <section className="an-group">
      <span className="orch-eyebrow">{title}</span>
      <div className="an-rows">
        {rows.map((row) => {
          const body = (
            <>
              <span className="an-row-label">{row.label}</span>
              {row.note && (
                <span
                  className={`an-note${row.note.tone ? ` ${row.note.tone}` : ""}`}
                >
                  {row.note.text}
                </span>
              )}
              <span className="an-row-value">{row.value}</span>
              {row.onOpen && <ChevronRight size={12} className="an-chevron" />}
            </>
          );
          return row.onOpen ? (
            <button key={row.label} className="an-row" onClick={row.onOpen}>
              {body}
            </button>
          ) : (
            <div key={row.label} className="an-row">
              {body}
            </div>
          );
        })}
      </div>
    </section>
  );
}

function SpendChart({ bars }: { bars: ReturnType<typeof spendBars> }) {
  const max = Math.max(...bars.map((b) => b.costUsd));
  const total = bars.reduce((sum, b) => sum + b.costUsd, 0);
  return (
    <section className="an-chart">
      <div className="an-chart-head">
        <span className="orch-eyebrow">SPEND PER DAY</span>
        <span className="an-chart-avg">
          {formatCost(total / bars.length)} a day on average
        </span>
      </div>
      <div className="an-plot">
        {bars.map((bar, i) => {
          const last = i === bars.length - 1 && bar.label === "Today";
          return (
            <div key={bar.key} className={`an-col${last ? " today" : ""}`}>
              <span className="an-col-value">{formatCost(bar.costUsd)}</span>
              <span
                className="an-bar"
                style={{
                  height: max > 0 ? Math.max(3, (bar.costUsd / max) * 56) : 3,
                }}
              />
              <span className="an-col-day">{bar.label}</span>
            </div>
          );
        })}
      </div>
    </section>
  );
}

/** The Analytics view: one week of work, attention and spend. `refresh`
 * changes whenever the task list does. `onOpenTask` enables the drill-down
 * chevrons that lead to a task. */
export function AnalyticsView({
  cwd,
  refresh,
  onOpenTask,
}: {
  cwd: string;
  refresh: string;
  onOpenTask?: (id: string) => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [offset, setOffset] = useState(0);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [spend, setSpend] = useState<Spend | null>(null);
  const [previous, setPrevious] = useState<RangedSummary | null>(null);
  const [landing, setLanding] = useState(false);
  const [landError, setLandError] = useState("");
  const period = useMemo(() => periodAt(Date.now(), offset), [offset]);

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .taskList(cwd, true)
      .then((list) => !cancelled && setTasks(list))
      .catch(() => undefined);
    loadSpend(orchestratorClient, cwd, period, offset)
      .then((result) => {
        if (cancelled) return;
        setSpend(result);
        if (!result.ranged) return setPrevious(null);
        loadPrevious(orchestratorClient, cwd, period).then(
          (p) => !cancelled && setPrevious(p),
        );
      })
      .catch(() => !cancelled && setSpend(null));
    return () => {
      cancelled = true;
    };
  }, [cwd, period, offset, refresh]);

  const now = useMemo(() => periodStats(tasks, period), [tasks, period]);
  const before = useMemo<PeriodStats | null>(
    () => (previous ? periodStats(tasks, periodAt(period.to - 1, 1)) : null),
    [tasks, period, previous],
  );
  const inPeriod = useMemo(() => cohort(tasks, period), [tasks, period]);
  const waiting = landTasks(tasks);

  const total = spend?.day.totals.costUsd ?? null;
  const previousTotal = previous?.totals.costUsd ?? null;
  const spendDelta =
    total !== null && previousTotal !== null
      ? delta(total - previousTotal, formatCost)
      : undefined;
  const perTask = total !== null && now.started ? total / now.started : null;
  const previousPerTask =
    previousTotal !== null && before?.started
      ? previousTotal / before.started
      : null;

  const open = (id: string | undefined) =>
    onOpenTask && id ? () => onOpenTask(id) : undefined;
  const firstOf = (status: Task["status"]) =>
    open(inPeriod.find((t) => t.status === status)?.id);

  const landAll = () => {
    setLanding(true);
    setLandError("");
    void waiting
      .reduce(
        (chain, task) =>
          chain.then(() => orchestratorClient.taskLand(task.id).then(() => {})),
        Promise.resolve(),
      )
      .catch((e) => setLandError(errorText(e)))
      .finally(() => setLanding(false));
  };

  const topStage = [...(spend?.stage.rows ?? [])].sort(
    (a, b) => b.costUsd - a.costUsd,
  )[0];
  const topTask = [...(spend?.task.rows ?? [])].sort(
    (a, b) => b.costUsd - a.costUsd,
  )[0];
  const topTaskTitle = topTask
    ? (tasks.find((t) => t.id === topTask.key)?.title ?? topTask.key)
    : "";
  const hasCost = total !== null && (spend?.day.totals.runs ?? 0) > 0;
  const endsToday = offset === 0;

  const summaryLine = [
    `${now.landed} of ${now.started} tasks landed`,
    ...(now.waitingToLand ? [`${now.waitingToLand} waiting to land`] : []),
  ].join(", ");

  const taskRows: RowData[] = [
    {
      label: "Started",
      value: String(now.started),
      note: before
        ? delta(now.started - before.started, String, true)
        : undefined,
    },
    {
      label: "Landed",
      value: String(now.landed),
      note:
        now.landedPercent === null
          ? undefined
          : { text: `${now.landedPercent}%` },
    },
    {
      label: "Waiting to land",
      value: String(now.waitingToLand),
      onOpen: open(waiting[0]?.id),
    },
    {
      label: "Failed",
      value: String(now.failed),
      note: now.failed
        ? { text: now.failedBy.map((f) => `${f.kind} ${f.count}`).join(" · ") }
        : undefined,
      onOpen: firstOf("failed"),
    },
    ...(now.medianToLandMs === null
      ? []
      : [
          {
            label: "Median start → landed",
            value: shortDuration(now.medianToLandMs),
          },
        ]),
  ];

  const attentionRows: RowData[] = [
    ...(now.hasQuestionHistory
      ? [{ label: "Questions asked", value: String(now.questions) }]
      : []),
    {
      label: "Answered for you by the orchestrator",
      value: String(now.answeredForYou),
      note: now.overturned
        ? { text: `you overturned ${now.overturned}` }
        : undefined,
    },
    ...(now.medianWaitMs === null
      ? []
      : [
          {
            label: "Median wait for your answer",
            value: shortDuration(now.medianWaitMs),
            note:
              before?.medianWaitMs != null
                ? delta(now.medianWaitMs - before.medianWaitMs, shortDuration)
                : undefined,
          },
        ]),
  ];

  const costRows: RowData[] = !hasCost
    ? []
    : [
        ...(perTask === null
          ? []
          : [
              {
                label: "Per task",
                value: formatCost(perTask),
                note:
                  previousPerTask === null
                    ? undefined
                    : delta(perTask - previousPerTask, formatCost),
              },
            ]),
        ...(topStage && total
          ? [
              {
                label: "Most on",
                value: topStage.key.replace(/_/g, " "),
                note: {
                  text: `${formatCost(topStage.costUsd)} of ${formatCost(total)}`,
                },
              },
            ]
          : []),
        ...(topTask && topTask.costUsd > 0
          ? [
              {
                label: "Most expensive task",
                value: topTaskTitle,
                note: { text: formatCost(topTask.costUsd) },
                onOpen: open(topTask.key),
              },
            ]
          : []),
      ];

  const bars = spend ? spendBars(spend.day, period, endsToday) : [];
  const canPage = spend?.ranged ?? false;

  return (
    <div className="orch-view-scroll an-view">
      <div className="an-head">
        <div className="an-titles">
          <h2 className="orch-view-title">Analytics</h2>
          <p className="an-summary">
            {summaryLine}
            {total !== null && ` · ${formatCost(total)} spent`}
          </p>
        </div>
        <div className="an-period" role="group" aria-label="Period">
          {canPage && (
            <button
              aria-label="Previous period"
              onClick={() => setOffset(offset + 1)}
            >
              <ChevronLeft size={14} />
            </button>
          )}
          <span>
            {endsToday ? "This week · " : ""}
            {periodRange(period)}
          </span>
          {canPage && (
            <button
              aria-label="Next period"
              disabled={offset === 0}
              onClick={() => setOffset(offset - 1)}
            >
              <ChevronRight size={14} />
            </button>
          )}
        </div>
      </div>

      <div className="an-strip">
        <Cell
          label="Landed"
          value={`${now.landed} of ${now.started}`}
          note={
            now.landedPercent === null
              ? undefined
              : {
                  text: [
                    `${now.landedPercent}%`,
                    before?.landedPercent != null
                      ? signed(
                          now.landedPercent - before.landedPercent,
                          (n) => `${n} pts`,
                        )
                      : null,
                  ]
                    .filter(Boolean)
                    .join(" · "),
                }
          }
        />
        {now.medianToLandMs !== null && (
          <Cell
            label="Median to land"
            value={shortDuration(now.medianToLandMs)}
            note={
              before?.medianToLandMs != null
                ? delta(
                    now.medianToLandMs - before.medianToLandMs,
                    shortDuration,
                  )
                : undefined
            }
          />
        )}
        <Cell
          label="Questions to you"
          value={String(now.questions)}
          note={
            now.answeredForYou
              ? { text: `${now.answeredForYou} answered for you` }
              : undefined
          }
        />
        {total !== null && (
          <Cell label="Spend" value={formatCost(total)} note={spendDelta} />
        )}
      </div>

      {waiting.length > 0 && (
        <div className="an-insight">
          <ArrowRight size={14} />
          <span>
            {waiting.length} finished{" "}
            {waiting.length === 1 ? "task waits" : "tasks wait"} for a clean
            checkout
          </span>
          <button
            className="ui-button secondary"
            disabled={landing}
            onClick={landAll}
          >
            <GitMerge size={14} />
            Land {waiting.length}
          </button>
        </div>
      )}
      {landError && (
        <p className="an-error" role="alert">
          {landError}
        </p>
      )}

      {hasCost && <SpendChart bars={bars} />}

      <Rows title="TASKS" rows={taskRows} />
      <Rows title="YOUR ATTENTION" rows={attentionRows} />
      <Rows title="COST" rows={costRows} />
    </div>
  );
}
