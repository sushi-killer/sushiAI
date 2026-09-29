import { useEffect, useState } from "react";
import { ArrowRight, Check, FileText, Terminal, X } from "lucide-react";
import { orchestratorClient } from "./client";
import {
  formatCost,
  formatDuration,
  orderedStageRows,
  stageLabel,
} from "./helpers";
import {
  barLabel,
  defaultSegment,
  segmentFacts,
  segmentLabel,
  segmentNarration,
  stageRows,
  type NarrationKind,
} from "./taskDetailModel";
import { Tag } from "./ui";
import type { SpendSummary, Task, TimelineSegment } from "./types";

function segmentTitle(segment: TimelineSegment): string {
  return [segmentLabel(segment), segmentFacts(segment), segment.outcome]
    .filter(Boolean)
    .join(" · ");
}

function NarrationIcon({ kind }: { kind: NarrationKind }) {
  const props = { size: 13, "aria-hidden": true };
  switch (kind) {
    case "run":
      return <Terminal {...props} />;
    case "ok":
      return <Check {...props} />;
    case "fail":
      return <X {...props} />;
    case "next":
      return <ArrowRight {...props} />;
    case "note":
      return <FileText {...props} />;
  }
}

/** A task's time: one bar with a segment per stage sized by duration, the
 * card of whichever segment is picked, and a row per stage with its runs,
 * time and cost. Fetched from the daemon, which derives it from the task
 * record and the runs' events. */
export function TaskTimeline({ task }: { task: Task }) {
  const [segments, setSegments] = useState<TimelineSegment[]>([]);
  const [picked, setPicked] = useState<{ id: string; index: number | null }>();

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .taskTimeline(task.id)
      .then((loaded) => !cancelled && setSegments(loaded))
      .catch(() => !cancelled && setSegments([]));
    return () => {
      cancelled = true;
    };
  }, [task.id, task.updatedAt]);

  if (segments.length === 0) return null;
  const index =
    picked && picked.id === task.id ? picked.index : defaultSegment(segments);
  const segment = index === null ? undefined : segments[index];
  const totalMs = segments.reduce(
    (sum, s) => sum + Math.max(0, s.endedAt - s.startedAt),
    0,
  );
  const rows = stageRows(task, segments);
  return (
    <section className="td-timeline" aria-label="Timeline">
      <div className="td-section-head">
        <span className="td-eyebrow">TIMELINE</span>
        <span className="td-faint">
          {formatDuration(totalMs)} · click a segment to see what the agent did
        </span>
      </div>
      <div className="td-bar" role="group" aria-label="Task timeline">
        {segments.map((s, i) => (
          <button
            key={i}
            type="button"
            className={`td-seg td-stage-${s.stage}${s.failureKind ? " failed" : ""}${i === index ? " picked" : ""}`}
            style={{ flexGrow: Math.max(1, s.endedAt - s.startedAt) }}
            title={segmentTitle(s)}
            aria-label={segmentTitle(s)}
            aria-pressed={i === index}
            onClick={() =>
              setPicked({ id: task.id, index: i === index ? null : i })
            }
          >
            {barLabel(s, totalMs)}
          </button>
        ))}
      </div>
      {segment && (
        <div className="td-seg-card">
          <div className="td-seg-card-head">
            <span className="td-seg-card-title">{segmentLabel(segment)}</span>
            {segment.failureKind && <Tag tone="danger">failed</Tag>}
            <span className="td-faint">{segmentFacts(segment)}</span>
          </div>
          {segmentNarration(task, segment).map((step, i) => (
            <p key={i} className={`td-step ${step.kind}`}>
              <NarrationIcon kind={step.kind} />
              <span>{step.text}</span>
            </p>
          ))}
        </div>
      )}
      <div className="td-stages">
        {rows.map((row) => (
          <div key={row.stage} className="td-stage-row">
            <span className={`td-swatch td-stage-${row.stage}`} />
            <span className="td-stage-name">{row.label}</span>
            <span className="td-stage-detail">{row.detail}</span>
            <span className="td-stage-time">{formatDuration(row.ms)}</span>
            <span className="td-stage-cost">
              {row.stage === "wait" ? "—" : formatCost(row.costUsd)}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

/** A task's cost by stage, from its per-run cost records, so review,
 * advisor, pick and the rest are each named. A task with no records yet
 * (recorded before they existed and not backfilled) shows `legacy`, the
 * breakdown derived from its attempts. */
export function TaskCostLine({
  task,
  subtasks,
  legacy,
}: {
  task: Task;
  subtasks: number;
  legacy: { label: string; cost: number }[];
}) {
  const [rows, setRows] = useState<SpendSummary["rows"]>([]);

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .costsSummary({ taskId: task.id, groupBy: ["stage"] })
      .then((loaded) => !cancelled && setRows(loaded.rows))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [task.id, task.updatedAt, task.costUsd]);

  const parts =
    rows.length > 0
      ? [
          ...orderedStageRows(rows).map((row) => ({
            label: stageLabel(row.key),
            cost: row.costUsd,
          })),
          ...(formatCost(subtasks) === "$0.00"
            ? []
            : [{ label: "Subtasks", cost: subtasks }]),
        ]
      : legacy;
  if (parts.length === 0) return null;
  return (
    <p
      className="td-facts td-cost-breakdown"
      title="Where this task's cost went, by stage"
    >
      {parts.map((row) => `${row.label} ${formatCost(row.cost)}`).join(" · ")}
    </p>
  );
}
