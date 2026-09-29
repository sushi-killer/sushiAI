import type { Tone } from "../helpers";

/** Orch/TaskRow: dot, title, a toned reason and a muted meta line. */
export function TaskRow({
  title,
  reason,
  meta,
  tone,
  selected,
  onClick,
}: {
  title: string;
  reason: string;
  meta: string;
  tone: Tone;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`ui-task-row${selected ? " selected" : ""}`}
      aria-current={selected || undefined}
      onClick={onClick}
    >
      <span className="ui-dot-wrap">
        <span className={`ui-dot ui-tone-${tone}`} />
      </span>
      <span className="ui-task-row-body">
        <span className="ui-task-row-title">{title}</span>
        {reason && (
          <span className={`ui-task-row-reason ui-tone-${tone}`}>{reason}</span>
        )}
        {meta && <span className="ui-task-row-meta">{meta}</span>}
      </span>
    </button>
  );
}
