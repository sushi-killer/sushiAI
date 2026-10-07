import type { ReactNode } from "react";
import type { Tone } from "./tone.ts";

/** Attention/Item: one thing that needs the owner - a toned marker, the
 * title and its age, a context line, the actions and a muted meta line.
 * `question` sets the context as the question itself (larger, full
 * contrast) rather than a muted summary. */
export function AttentionItem({
  tone,
  title,
  icon,
  time,
  context,
  question = false,
  actions,
  meta,
  selected = false,
  onOpen,
}: {
  tone: Tone;
  title: string;
  /** Provenance mark drawn before the title. */
  icon?: ReactNode;
  time?: string;
  context?: ReactNode;
  question?: boolean;
  actions?: ReactNode;
  meta?: string;
  selected?: boolean;
  onOpen?: () => void;
}) {
  return (
    <article className={`ui-attention${selected ? " selected" : ""}`}>
      <span className="ui-attention-marker">
        <span className={`ui-attention-dot ui-tone-${tone}`} />
      </span>
      <div className="ui-attention-body">
        <div className="ui-attention-head">
          {icon && <span className="ui-attention-icon">{icon}</span>}
          {onOpen ? (
            <button
              type="button"
              className="ui-attention-title"
              onClick={onOpen}
            >
              {title}
            </button>
          ) : (
            <span className="ui-attention-title">{title}</span>
          )}
          {time && <span className="ui-attention-time">{time}</span>}
        </div>
        {context && (
          <div className={`ui-attention-context${question ? " question" : ""}`}>
            {context}
          </div>
        )}
        {actions && <div className="ui-attention-actions">{actions}</div>}
        {meta && <span className="ui-attention-meta">{meta}</span>}
      </div>
    </article>
  );
}
