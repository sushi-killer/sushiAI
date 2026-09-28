import { useEffect, useState } from "react";
import { orchestratorClient } from "./client";
import { formatCost, formatDuration } from "./helpers";
import type { FailureRow, Task, TimelineSegment } from "./types";

const TOP_FAILURES = 10;

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

/** The failures that keep coming back across this repo's tasks, most
 * frequent first; a row opens the task where it was seen last. `refresh`
 * changes whenever the task list does. */
export function RecurringFailures({
  cwd,
  refresh,
  onOpenTask,
}: {
  cwd: string;
  refresh: string;
  onOpenTask: (id: string) => void;
}) {
  const [rows, setRows] = useState<FailureRow[]>([]);

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .failuresCatalogue(cwd)
      .then((loaded) => !cancelled && setRows(loaded))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh]);

  const top = rows.filter((row) => row.count > 1).slice(0, TOP_FAILURES);
  if (top.length === 0) return null;
  return (
    <div className="orch-failures">
      <span className="dialog-eyebrow">RECURRING FAILURES</span>
      {top.map((row) => (
        <button
          key={row.signature}
          className="orch-failure-row"
          title={row.exampleDetail}
          onClick={() => onOpenTask(row.exampleTaskId)}
        >
          <span className="orch-failure-count">{row.count}×</span>
          <span className="orch-failure-kind">{row.kind}</span>
          {/* The signature has its digits stripped for grouping; show a real one. */}
          <span className="orch-failure-signature">
            {row.exampleDetail.split("\n")[0] || row.signature}
          </span>
          <span className="orch-failure-tasks">
            {row.tasks.length} {row.tasks.length === 1 ? "task" : "tasks"}
          </span>
        </button>
      ))}
    </div>
  );
}
