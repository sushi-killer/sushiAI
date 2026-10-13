import { useEffect, useMemo, useState } from "react";
import { ArrowRight, ChevronLeft, ChevronRight } from "lucide-react";
import {
  clientFor,
  useOrchestratorClient,
  useOrchestratorHost,
} from "./hostContext";
import {
  loadProjectSpend,
  loadProjectTasks,
  type ProjectSource,
} from "./projectScope";
import { useProjectScope } from "./useProjectScope";
import { errorText, formatCost, formatDuration } from "./helpers";
import { landTasks } from "./ownerAttention";
import {
  cohort,
  landInsight,
  landingTasks,
  periodAt,
  periodStats,
  signed,
  spendBars,
  spendRange,
  type Period,
  type PeriodStats,
} from "./stats";
import type { SpendGroup, SpendSummary, Task } from "./types";
import "./analytics.css";

type Spend = {
  day: SpendSummary;
  stage: SpendSummary;
  task: SpendSummary;
};

type Tone = "ok" | "warning";
type Note = { text: string; tone?: Tone };

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

/** The period's spend by day, stage and task, windowed by its UTC dates. */
async function loadSpend(
  sources: ProjectSource[],
  period: Period,
): Promise<Spend & { failed: string[] }> {
  const query = (groupBy: SpendGroup) =>
    loadProjectSpend(clientFor, sources, {
      ...spendRange(period),
      groupBy: [groupBy],
    });
  const [day, stage, task] = await Promise.all([
    query("day"),
    query("stage"),
    query("task"),
  ]);
  const failed = [...new Set([...day.failed, ...stage.failed, ...task.failed])];
  return { day: day.summary, stage: stage.summary, task: task.summary, failed };
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
  const host = useOrchestratorHost();
  const { sources, hostName } = useProjectScope(cwd);
  const [offset, setOffset] = useState(0);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [spend, setSpend] = useState<Spend | null>(null);
  const [previous, setPrevious] = useState<SpendSummary | null>(null);
  const [spendError, setSpendError] = useState("");
  const [unreached, setUnreached] = useState<string[]>([]);
  const [landing, setLanding] = useState(false);
  const [landError, setLandError] = useState("");
  const period = useMemo(() => periodAt(Date.now(), offset), [offset]);

  useEffect(() => {
    let cancelled = false;
    const report = (failed: string[]) =>
      setUnreached((old) => [...new Set([...old, ...failed])]);
    setUnreached([]);
    loadProjectTasks(clientFor, sources, true)
      .then((result) => {
        if (cancelled) return;
        setTasks(result.tasks);
        report(result.failed);
      })
      .catch(() => undefined);
    loadSpend(sources, period)
      .then((result) => {
        if (cancelled) return;
        setSpend(result);
        setSpendError("");
        report(result.failed);
      })
      .catch((e) => {
        if (cancelled) return;
        setSpend(null);
        setSpendError(errorText(e));
      });
    loadProjectSpend(clientFor, sources, {
      ...spendRange(periodAt(period.to - 1, 1)),
      groupBy: ["day"],
    })
      .then((result) => {
        if (cancelled) return;
        setPrevious(result.summary);
        report(result.failed);
      })
      .catch(() => !cancelled && setPrevious(null));
    return () => {
      cancelled = true;
    };
  }, [sources, period, refresh]);

  const now = useMemo(() => periodStats(tasks, period), [tasks, period]);
  const before = useMemo<PeriodStats>(
    () => periodStats(tasks, periodAt(period.to - 1, 1)),
    [tasks, period],
  );
  const inPeriod = useMemo(() => cohort(tasks, period), [tasks, period]);
  // Only this host's tasks can be opened or landed from here.
  const own = useMemo(
    () => tasks.filter((t) => (t.host || "local") === host),
    [tasks, host],
  );
  const waiting = landTasks(own);
  const insight = landInsight(waiting.length, landingTasks(own));

  const total = spend?.day.totals.costUsd ?? null;
  const previousTotal = previous?.totals.costUsd ?? null;
  const spendDelta =
    total !== null && previousTotal !== null
      ? delta(total - previousTotal, formatCost)
      : undefined;
  const perTask = total !== null && now.started ? total / now.started : null;
  const previousPerTask =
    previousTotal !== null && before.started
      ? previousTotal / before.started
      : null;

  const open = (id: string | undefined) =>
    onOpenTask && id && own.some((t) => t.id === id)
      ? () => onOpenTask(id)
      : undefined;
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
      note: delta(now.started - before.started, String, true),
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
            value: formatDuration(now.medianToLandMs),
          },
        ]),
  ];

  const attentionRows: RowData[] = [
    {
      label: "Questions asked",
      value: String(now.questions),
      note: delta(now.questions - before.questions, String),
    },
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
            value: formatDuration(now.medianWaitMs),
            note:
              before.medianWaitMs !== null
                ? delta(now.medianWaitMs - before.medianWaitMs, formatDuration)
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
          <button
            aria-label="Previous period"
            onClick={() => setOffset(offset + 1)}
          >
            <ChevronLeft size={14} />
          </button>
          <span>
            {endsToday ? "This week · " : ""}
            {periodRange(period)}
          </span>
          <button
            aria-label="Next period"
            disabled={offset === 0}
            onClick={() => setOffset(offset - 1)}
          >
            <ChevronRight size={14} />
          </button>
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
                    before.landedPercent !== null
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
        <Cell
          label="Median to land"
          value={
            now.medianToLandMs === null
              ? "—"
              : formatDuration(now.medianToLandMs)
          }
          note={
            now.medianToLandMs !== null && before.medianToLandMs !== null
              ? delta(
                  now.medianToLandMs - before.medianToLandMs,
                  formatDuration,
                )
              : undefined
          }
        />
        <Cell
          label="Questions to you"
          value={String(now.questions)}
          note={
            now.answeredForYou
              ? { text: `${now.answeredForYou} answered for you` }
              : undefined
          }
        />
        <Cell
          label="Spend"
          value={total === null ? "—" : formatCost(total)}
          note={spendDelta}
        />
      </div>

      {insight && (
        <div className="an-insight">
          <ArrowRight size={14} />
          <span>{insight}</span>
          {waiting.length > 0 && (
            <button
              className="ui-button secondary"
              disabled={landing}
              onClick={landAll}
            >
              Land {waiting.length}
            </button>
          )}
        </div>
      )}
      {spendError && (
        <p className="an-error" role="alert">
          Spend: {spendError}
        </p>
      )}
      {unreached.length > 0 && (
        <p className="an-note warning">
          Not counted: {unreached.map((id) => hostName(id)).join(", ")}{" "}
          (unreachable)
        </p>
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
