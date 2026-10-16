import { useLayoutEffect, useRef, useState } from "react";
import { Chip } from "../ui/index.ts";
import { composerHeight } from "../ui/composerSize.ts";
import {
  initialAnswers,
  isAnswered,
  type Ask,
  type AskAction,
  type AskAnswer,
  type AskAnswers,
} from "./model.ts";

/** The text answer: a full-width field that grows with what is typed. */
function AskField({
  value,
  label,
  labelledBy,
  placeholder,
  disabled,
  autoFocus,
  onChange,
}: {
  value: string;
  label?: string;
  labelledBy?: string;
  placeholder?: string;
  disabled: boolean;
  autoFocus?: boolean;
  onChange(value: string): void;
}) {
  const field = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const node = field.current;
    if (!node) return;
    node.style.height = "auto";
    const style = getComputedStyle(node);
    const next = composerHeight(
      node.scrollHeight,
      parseFloat(style.lineHeight) || 18,
      parseFloat(style.paddingTop) + parseFloat(style.paddingBottom),
      5,
    );
    node.style.height = `${next.height}px`;
    node.style.overflowY = next.overflow ? "auto" : "hidden";
  }, [value]);
  return (
    <textarea
      ref={field}
      className="rt-ask-input"
      rows={1}
      aria-label={label}
      aria-labelledby={labelledBy}
      placeholder={placeholder}
      value={value}
      disabled={disabled}
      autoFocus={autoFocus}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

/** The builder's question under its message: buttons for a choice, chips for
 * several, a field for text. Active only for the newest message; afterwards it
 * shows what was answered, read-only. */
export function AskCard({
  ask,
  answered,
  active,
  onSend,
  onAction,
  disabledKinds = [],
}: {
  ask: Ask;
  /** The answers the owner gave this card, when they used it. */
  answered?: AskAnswers;
  active: boolean;
  onSend(answers: AskAnswers): void;
  /** A button of the whitelist; the view decides if it can run. */
  onAction(action: AskAction): void;
  /** Kinds that cannot run now (no draft yet). */
  disabledKinds?: AskAction["kind"][];
}) {
  const [draft, setDraft] = useState<AskAnswers>(() => initialAnswers(ask));
  const [other, setOther] = useState<Record<string, boolean>>({});
  const shown = active ? draft : (answered ?? {});
  const lone = ask.questions.length === 1 ? ask.questions[0] : null;
  // One plain choice sends on the click; everything else has a Send button.
  const immediate = !!lone && lone.type === "choice" && !lone.allowOther;
  const hasQuestions = ask.questions.length > 0;
  const ready = ask.questions.every((one) => isAnswered(one, draft[one.id]));
  const patch = (id: string, change: Partial<AskAnswer>) =>
    setDraft((now) => ({ ...now, [id]: { ...now[id], ...change } }));

  return (
    <div
      className="rt-ask"
      role="group"
      aria-label={ask.title || "Question"}
      aria-disabled={!active}
    >
      {ask.title && <strong className="rt-ask-title">{ask.title}</strong>}
      {ask.questions.map((question) => {
        const answer = shown[question.id];
        const typed = answer?.other.trim() ?? "";
        const open = active ? !!other[question.id] : !!typed;
        return (
          <div key={question.id} className="rt-ask-row">
            <span className="rt-ask-label" id={`ask-${question.id}`}>
              {question.label}
            </span>
            {question.type === "text" ? (
              <AskField
                labelledBy={`ask-${question.id}`}
                placeholder={question.placeholder}
                value={answer?.other ?? ""}
                disabled={!active}
                onChange={(other) => patch(question.id, { other })}
              />
            ) : (
              <div
                className="rt-ask-options"
                role={question.type === "choice" ? "radiogroup" : "group"}
                aria-labelledby={`ask-${question.id}`}
              >
                {question.options.map((option) => (
                  <Chip
                    key={option.value}
                    radio={question.type === "choice"}
                    disabled={!active}
                    selected={answer?.picked.includes(option.value) ?? false}
                    onClick={() => {
                      const was = answer?.picked ?? [];
                      const picked =
                        question.type === "choice"
                          ? [option.value]
                          : was.includes(option.value)
                            ? was.filter((one) => one !== option.value)
                            : [...was, option.value];
                      const next = {
                        ...draft,
                        [question.id]: { picked, other: "" },
                      };
                      setDraft(next);
                      setOther((now) => ({ ...now, [question.id]: false }));
                      if (immediate) onSend(next);
                    }}
                  >
                    {option.label}
                  </Chip>
                ))}
                {question.allowOther && (
                  <Chip
                    className="other"
                    radio={question.type === "choice"}
                    disabled={!active}
                    selected={open}
                    onClick={() => {
                      setOther((now) => ({ ...now, [question.id]: true }));
                      if (question.type === "choice")
                        patch(question.id, { picked: [] });
                    }}
                  >
                    Other…
                  </Chip>
                )}
              </div>
            )}
            {question.allowOther && open && (
              <AskField
                label={`${question.label}: other`}
                placeholder={question.placeholder || "Your answer"}
                value={answer?.other ?? ""}
                disabled={!active}
                autoFocus={active}
                onChange={(other) => patch(question.id, { other })}
              />
            )}
          </div>
        );
      })}
      {active && hasQuestions && !immediate && (
        <div className="rt-ask-actions">
          {!ready && (
            <span className="rt-ask-hint">Answer every question to send.</span>
          )}
          <button
            type="button"
            className="ui-button primary"
            disabled={!ready}
            onClick={() => onSend(draft)}
          >
            {lone ? "Send" : "Send answers"}
          </button>
        </div>
      )}
      {!active && answered && hasQuestions && (
        <span className="rt-ask-hint done">Answers sent.</span>
      )}
      {ask.actions.length > 0 && (
        <div className="rt-ask-actions">
          {ask.actions.map((action) => (
            <button
              key={action.id}
              type="button"
              className={`ui-button ${action.style}`}
              disabled={!active || disabledKinds.includes(action.kind)}
              onClick={() => onAction(action)}
            >
              {action.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
