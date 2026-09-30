import { Fragment } from "react";
import { Check, X } from "lucide-react";
import type { StageStep } from "../helpers";

/** Orch/StageTrack: brief → implement → verify → review → land, each step a
 * marker and a label, joined by equal connectors that turn green after a
 * done step. As its width shrinks it tightens the gaps, then keeps only the
 * current step's label, then only the markers (ui.css container queries);
 * every label stays the step's accessible name. */
export function StageTrack({ steps }: { steps: StageStep[] }) {
  return (
    <div className="ui-stage-frame">
      <div className="ui-stage-track" role="list" aria-label="Stages">
        {steps.map((step, index) => (
          <Fragment key={step.stage}>
            {index > 0 && (
              <span
                className={`ui-stage-connector${steps[index - 1].state === "done" ? " done" : ""}`}
                aria-hidden
              />
            )}
            <span
              className={`ui-stage-step ${step.state}`}
              role="listitem"
              title={step.stage}
            >
              <span className="ui-stage-marker" aria-hidden>
                {step.state === "done" && <Check size={11} />}
                {step.state === "active" && <span className="ui-stage-pip" />}
                {step.state === "blocked" && "?"}
                {step.state === "failed" && <X size={11} />}
              </span>
              <span className="ui-stage-label">{step.stage}</span>
            </span>
          </Fragment>
        ))}
      </div>
    </div>
  );
}
