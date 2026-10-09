import { useEffect, useRef, useState } from "react";
import {
  ALLOW_DELAY_MS,
  escapeText,
  execKeyAction,
  showWord,
} from "./companionExec.ts";

export type ExecQuestion = Parameters<
  Parameters<NonNullable<Window["bridge"]>["onCompanionExec"]>[0]
>[0];

/** The card for one `host.exec` call. Everything the extension supplied is
 * escaped, so nothing hides in it. It shows the exact argv, the extension's
 * own words as a description (not as fact) and, for stdin, only its size and
 * SHA-256. Deny is the default; Allow needs a click once `ready`. */
export function ExecCard({
  question,
  ready,
  onAnswer,
}: {
  question: ExecQuestion;
  ready: boolean;
  onAnswer(allow: boolean): void;
}) {
  const root = useRef<HTMLDivElement>(null);
  return (
    <div className="modal-backdrop">
      <div
        ref={root}
        className="modal extension-approval companion-exec"
        role="alertdialog"
        aria-modal="true"
        aria-label={`${escapeText(question.extensionName)} wants to run a command`}
        onKeyDown={(event) => {
          if (event.key === "Tab") {
            // Focus stays inside the card.
            const items = [
              ...(root.current?.querySelectorAll<HTMLElement>(
                "[tabindex='0'], button:not(:disabled)",
              ) ?? []),
            ];
            if (!items.length) return;
            const at = items.indexOf(document.activeElement as HTMLElement);
            const step = event.shiftKey ? -1 : 1;
            event.preventDefault();
            items[(at + step + items.length) % items.length].focus();
            return;
          }
          const action = execKeyAction(
            event.key,
            (event.target as HTMLElement).dataset.allow === "true",
          );
          if (action === "default") return;
          event.preventDefault();
          if (action === "deny") onAnswer(false);
        }}
      >
        <h2>Run a command on {escapeText(question.hostName)}?</h2>
        <dl className="extension-approval-facts">
          <dt>Extension</dt>
          <dd>
            {escapeText(question.extensionName)}{" "}
            <code>{escapeText(question.extensionId)}</code>
          </dd>
          <dt>Host</dt>
          <dd>
            {escapeText(question.hostName)}{" "}
            <code>{escapeText(question.hostAddress)}</code>
          </dd>
          <dt>Extension&rsquo;s description</dt>
          <dd>{escapeText(question.title)}</dd>
          <dt>Input</dt>
          <dd>
            {question.stdinBytes
              ? `${question.stdinBytes} bytes · SHA-256 ${question.stdinSha256}`
              : "None"}
          </dd>
        </dl>
        <p className="companion-exec-label">Command, exactly as sent</p>
        <div className="companion-exec-argv" tabIndex={0}>
          {question.argv.map((word, index) => (
            <pre key={index}>{showWord(word)}</pre>
          ))}
        </div>
        <div className="extension-approval-actions">
          <button
            className="secondary"
            autoFocus
            onClick={() => onAnswer(false)}
          >
            Deny
          </button>
          <button
            className="primary"
            data-allow="true"
            disabled={!ready}
            onClick={() => onAnswer(true)}
          >
            Allow
          </button>
        </div>
      </div>
    </div>
  );
}

/** The card on screen, if main announced one. Main owns the queue and the
 * clock; this draws one card, drops it when main withdraws it, and keeps Allow
 * disabled for a second after the card appears and after the window regains
 * focus. */
export function CompanionExecPrompt() {
  const [question, setQuestion] = useState<ExecQuestion | null>(null);
  const [readyId, setReadyId] = useState<string | null>(null);
  const [focusTick, setFocusTick] = useState(0);
  useEffect(() => {
    const offs = [
      window.bridge?.onCompanionExec(setQuestion),
      window.bridge?.onCompanionExecWithdraw(({ id }) =>
        setQuestion((current) => (current?.id === id ? null : current)),
      ),
    ];
    const onFocus = () => setFocusTick((tick) => tick + 1);
    window.addEventListener("focus", onFocus);
    return () => {
      for (const off of offs) off?.();
      window.removeEventListener("focus", onFocus);
    };
  }, []);
  const id = question?.id;
  useEffect(() => {
    setReadyId(null);
    if (!id) return;
    const timer = window.setTimeout(() => setReadyId(id), ALLOW_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [id, focusTick]);
  if (!question) return null;
  return (
    <ExecCard
      question={question}
      ready={readyId === question.id}
      onAnswer={(allow) => {
        setQuestion(null);
        void window.bridge?.companionExecAnswer(question.id, allow);
      }}
    />
  );
}
