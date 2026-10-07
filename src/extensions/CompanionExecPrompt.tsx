import { useEffect, useState } from "react";
import { ALLOW_DELAY_MS, execKeyAction, showWord } from "./companionExec.ts";

type Question = Parameters<
  Parameters<NonNullable<Window["bridge"]>["onCompanionExec"]>[0]
>[0];

/** The owner card for one `host.exec` call. The app draws it, outside the
 * extension's tab. It shows the exact argv and the extension's own words as a
 * description, never as fact, and only the size and SHA-256 of stdin. Deny is
 * the default: Enter and Escape deny, and Allow needs a click after a short
 * delay. Questions queue; the oldest shows first. */
export function CompanionExecPrompt() {
  const [queue, setQueue] = useState<Question[]>([]);
  const [ready, setReady] = useState(false);
  useEffect(
    () =>
      window.bridge?.onCompanionExec((question) =>
        setQueue((current) => [...current, question]),
      ),
    [],
  );
  const question = queue[0];
  const id = question?.id;
  useEffect(() => {
    setReady(false);
    if (!id) return;
    const timer = window.setTimeout(() => setReady(true), ALLOW_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [id]);
  if (!question) return null;
  const answer = (allow: boolean) => {
    setQueue((current) => current.slice(1));
    void window.bridge?.companionExecAnswer(question.id, allow);
  };
  return (
    <div className="modal-backdrop">
      <div
        className="modal extension-approval companion-exec"
        role="alertdialog"
        aria-modal="true"
        aria-label={`${question.extensionName} wants to run a command`}
        onKeyDown={(event) => {
          const action = execKeyAction(
            event.key,
            (event.target as HTMLElement).dataset.allow === "true",
          );
          if (action === "default") return;
          event.preventDefault();
          if (action === "deny") answer(false);
        }}
      >
        <h2>Run a command on {question.hostName}?</h2>
        <dl className="extension-approval-facts">
          <dt>Extension</dt>
          <dd>
            {question.extensionName} <code>{question.extensionId}</code>
          </dd>
          <dt>Host</dt>
          <dd>
            {question.hostName} <code>{question.hostAddress}</code>
          </dd>
          <dt>Extension&rsquo;s description</dt>
          <dd>{question.title}</dd>
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
          <button className="secondary" autoFocus onClick={() => answer(false)}>
            Deny
          </button>
          <button
            className="primary"
            data-allow="true"
            disabled={!ready}
            onClick={() => answer(true)}
          >
            Allow
          </button>
        </div>
      </div>
    </div>
  );
}
