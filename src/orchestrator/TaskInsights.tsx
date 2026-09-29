import { useEffect, useState } from "react";
import { orchestratorClient } from "./client";
import {
  formatCost,
  formatDuration,
  orderedStageRows,
  stageLabel,
} from "./helpers";
import type { SpendSummary, Task, TimelineSegment } from "./types";

function segmentTitle(segment: TimelineSegment): string {
  const parts = [
    segment.stage === "wait" ? "waiting for you" : segment.stage,
    segment.attempt > 0 ? `attempt ${segment.attempt}` : "",
    formatDuration(segment.endedAt - segment.startedAt),
    formatCost(segment.costUsd),
    segment.outcome,
    segment.failureKind ?? "",
  ];
  return parts.filter(Boolean).join(" · ");
}

/** A task's time as one horizontal bar: a segment per stage, sized by
 * duration, the cost of each in its tooltip. Fetched from the daemon, which
 * derives it from the task record and the runs' events. */
export function TaskTimelineBar({ task }: { task: Task }) {
  const [segments, setSegments] = useState<TimelineSegment[]>([]);

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
  return (
    <div className="orch-timeline">
      <div className="orch-timeline-bar" role="img" aria-label="Task timeline">
        {segments.map((segment, index) => (
          <span
            key={index}
            className={`orch-timeline-segment stage-${segment.stage}${
              segment.failureKind ? " failed" : ""
            }`}
            style={{
              flexGrow: Math.max(1, segment.endedAt - segment.startedAt),
            }}
            title={segmentTitle(segment)}
          />
        ))}
      </div>
      <div className="orch-timeline-legend">
        {[...new Set(segments.map((s) => s.stage))].map((stage) => (
          <span key={stage} className="orch-timeline-key">
            <span className={`orch-timeline-swatch stage-${stage}`} />
            {stage === "wait" ? "waiting" : stage}
          </span>
        ))}
      </div>
    </div>
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
      className="orch-detail-meta orch-cost-breakdown"
      title="Where this task's cost went, by stage"
    >
      {parts.map((row) => `${row.label} ${formatCost(row.cost)}`).join(" · ")}
    </p>
  );
}
