import { Check, X } from "lucide-react";
import type { StageStep } from "../helpers";

/** Orch/StageTrack: brief → implement → verify → review → land, each step a
 * marker and a label joined by a connector that turns green after a done
 * step. */
export function StageTrack({ steps }: { steps: StageStep[] }) {
  return (
    <div className="ui-stage-track" role="list" aria-label="Stages">
      {steps.map((step, index) => (
        <span key={step.stage} className="ui-stage-slot" role="listitem">
          {index > 0 && (
            <span
              className={`ui-stage-connector${steps[index - 1].state === "done" ? " done" : ""}`}
            />
          )}
          <span className={`ui-stage-step ${step.state}`}>
            <span className="ui-stage-marker" aria-hidden>
              {step.state === "done" && <Check size={11} />}
              {step.state === "active" && <span className="ui-stage-pip" />}
              {step.state === "blocked" && "?"}
              {step.state === "failed" && <X size={11} />}
            </span>
            <span className="ui-stage-label">{step.stage}</span>
          </span>
        </span>
      ))}
    </div>
  );
}
