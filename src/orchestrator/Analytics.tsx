import { useEffect, useState } from "react";
import { orchestratorClient } from "./client";
import { formatCost, orderedStageRows, stageLabel } from "./helpers";
import type { SpendSummary } from "./types";

const SPEND_TOP_MODELS = 5;

function SpendBars({
  rows,
  total,
  label,
}: {
  rows: { key: string; costUsd: number; runs: number }[];
  total: number;
  label: (key: string) => string;
}) {
  return (
    <div className="orch-spend-list">
      {rows.map((row) => (
        <div
          key={row.key}
          className="orch-spend-row"
          title={`${row.runs} ${row.runs === 1 ? "run" : "runs"}`}
        >
          <span className="orch-spend-name">{label(row.key)}</span>
          <span className="orch-spend-bar">
            <span
              className="orch-spend-fill"
              style={{
                width: `${total > 0 ? (row.costUsd / total) * 100 : 0}%`,
              }}
            />
          </span>
          <span className="orch-spend-cost">{formatCost(row.costUsd)}</span>
        </div>
      ))}
    </div>
  );
}

/** What this repo's model runs cost over the last 7 or 30 days, by stage and
 * by model (top 5). `refresh` changes whenever the task list does. */
function SpendBlock({ cwd, refresh }: { cwd: string; refresh: string }) {
  const [days, setDays] = useState<7 | 30>(7);
  const [byStage, setByStage] = useState<SpendSummary | null>(null);
  const [byModel, setByModel] = useState<SpendSummary | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      orchestratorClient.costsSummary({
        repo: cwd,
        sinceDays: days,
        groupBy: ["stage"],
      }),
      orchestratorClient.costsSummary({
        repo: cwd,
        sinceDays: days,
        groupBy: ["model"],
      }),
    ])
      .then(([stage, model]) => {
        if (cancelled) return;
        setByStage(stage);
        setByModel(model);
      })
      .catch(() => {
        if (cancelled) return;
        setByStage(null);
        setByModel(null);
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, days, refresh]);

  if (!byStage || !byModel || byStage.totals.runs === 0) return null;
  const total = byStage.totals.costUsd;
  return (
    <div className="orch-spend">
      <div className="orch-spend-head">
        <span className="dialog-eyebrow">SPEND</span>
        <span className="orch-spend-total">
          {formatCost(total)} · last {days} days · {byStage.totals.runs} runs
        </span>
        <span className="orch-spend-toggle" role="group" aria-label="Period">
          {([7, 30] as const).map((option) => (
            <button
              key={option}
              className={option === days ? "active" : ""}
              aria-pressed={option === days}
              onClick={() => setDays(option)}
            >
              {option}d
            </button>
          ))}
        </span>
      </div>
      <div className="orch-spend-columns">
        <div>
          <span className="orch-spend-caption">By stage</span>
          <SpendBars
            rows={orderedStageRows(byStage.rows)}
            total={total}
            label={stageLabel}
          />
        </div>
        <div>
          <span className="orch-spend-caption">By model</span>
          <SpendBars
            rows={byModel.rows
              .filter((row) => formatCost(row.costUsd) !== "$0.00")
              .slice(0, SPEND_TOP_MODELS)}
            total={total}
            label={(key) => key}
          />
        </div>
      </div>
    </div>
  );
}

/** The Analytics view. For now the spend block; lane L4 builds the rest of
 * the Figma frame. `refresh` changes whenever the task list does. */
export function AnalyticsView({
  cwd,
  refresh,
}: {
  cwd: string;
  refresh: string;
}) {
  return (
    <div className="orch-view-scroll">
      <h2 className="orch-view-title">Analytics</h2>
      <SpendBlock cwd={cwd} refresh={refresh} />
    </div>
  );
}
