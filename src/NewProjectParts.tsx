import type { ReactNode } from "react";
import { Check } from "lucide-react";
import { Tag } from "./orchestrator/ui";
import type { Tone } from "./orchestrator/helpers";

/** One numbered step of the New project flow. A step that has not started is
 * drawn at half strength; the current one has a filled number. */
export function StepHead({
  n,
  title,
  current,
}: {
  n: number;
  title: string;
  current: boolean;
}) {
  return (
    <div
      className={`np-step${current ? " current" : ""}${n === 3 ? " last" : ""}`}
    >
      <span className="np-n">{n}</span>
      <strong>{title}</strong>
    </div>
  );
}

/** A finished step, folded to one line with a way back into it. */
export function DoneRow({
  title,
  summary,
  onEdit,
}: {
  title: string;
  summary: string;
  onEdit(): void;
}) {
  return (
    <div className="np-done">
      <span className="np-check">
        <Check size={11} aria-hidden />
      </span>
      <strong>{title}</strong>
      <span className="np-done-summary">{summary}</span>
      <button
        type="button"
        className="np-edit"
        aria-label={`Edit ${title.toLowerCase()}`}
        onClick={onEdit}
      >
        Edit
      </button>
    </div>
  );
}

/** The bottom line of every step: a note on the left, buttons on the right. */
export function StepFooter({
  note,
  className = "",
  children,
}: {
  note: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <footer className={`np-footer ${className}`}>
      <span className="np-note">{note}</span>
      {children}
    </footer>
  );
}

/** What one host is doing while the project is created on it. */
export type HostStep = {
  label: string;
  state: "done" | "working" | "pending" | "failed" | "warning";
  seconds?: number;
};

const clock = (seconds: number) =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;

/** One host's card in the Creating step: its name and path, its overall
 * state, then the steps it goes through. */
export function HostProgress({
  icon,
  name,
  path,
  tag,
  steps,
  retry,
  children,
}: {
  icon: ReactNode;
  name: string;
  path: string;
  tag: { label: string; tone: Tone };
  steps: HostStep[];
  retry?: {
    label: string;
    note?: string;
    disabled?: boolean;
    onRetry(): void;
  };
  children?: ReactNode;
}) {
  return (
    <section className="np-card" aria-label={name}>
      <div className="np-card-head">
        {icon}
        <strong>{name}</strong>
        <span className="np-card-path">{path}</span>
        <Tag tone={tag.tone}>{tag.label}</Tag>
      </div>
      {steps.map((step) => (
        <div className={`np-progress ${step.state}`} key={step.label}>
          <span className="np-mark" aria-hidden />
          <span className="np-progress-label">{step.label}</span>
          {step.seconds !== undefined && (
            <span className="np-progress-time">{clock(step.seconds)}</span>
          )}
        </div>
      ))}
      {retry && (
        <div className="np-retry">
          {retry.note && <span className="np-retry-text">{retry.note}</span>}
          <button
            type="button"
            className="ui-button secondary"
            disabled={retry.disabled}
            onClick={retry.onRetry}
          >
            {retry.label}
          </button>
        </div>
      )}
      {children}
    </section>
  );
}
