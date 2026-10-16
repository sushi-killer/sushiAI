import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ExternalLink } from "lucide-react";
import type { CoreViewProps } from "../extensions/coreViews.ts";
import { MarkdownDoc } from "../extensions/preview/MarkdownDoc.tsx";
import { Check, DocBody, Dot, Eyebrow, MetaLine, Rows } from "../ui/index.ts";
import { routinesClient } from "./client.ts";
import { LoginButton } from "./LoginButton.tsx";
import {
  actionCounts,
  approvalWords,
  durationText,
  failureText,
  fixCommand,
  leadingHeading,
  missingNotice,
  pendingActions,
  type PendingAction,
  runDateTime,
  safeLink,
} from "./model.ts";
import { sessionHostOf } from "./Scheme.tsx";
import type { RunResult, RunSummary } from "./types.ts";
import { useRunAsks, type OwnerQuestion } from "./useRunAsks.ts";

/** R5: the owner card of a `sushiai ask`. A pick or a typed answer goes back
 * through `ask.respond`; Skip declines it. */
export function ReviewCard({
  question,
  host,
  trust = "",
}: {
  question: OwnerQuestion;
  host: string;
  /** "2 more accepted runs and it acts on its own". */
  trust?: string;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function answer(
    decision: "allow" | "deny",
    extra: { choice?: string; message?: string } = {},
  ) {
    setBusy(true);
    setError("");
    try {
      await window.bridge!.askRespond(host, {
        sessionId: question.session,
        askId: question.askId,
        decision,
        ...extra,
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setBusy(false);
    }
  }
  return (
    <section className="rt-review" aria-label="Review before it acts">
      <Eyebrow>Wants your OK</Eyebrow>
      <Rows>
        <p className="rt-review-text">{question.text}</p>
      </Rows>
      <MetaLine>The run's own description of what it will do.</MetaLine>
      <div className="rt-review-actions">
        {question.options.map((option, index) => (
          <button
            key={option.id}
            type="button"
            className={`ui-button ${index === 0 ? "primary" : "secondary"}`}
            disabled={busy}
            onClick={() => void answer("allow", { choice: option.id })}
          >
            {option.label}
          </button>
        ))}
        <button
          type="button"
          className="ui-button ghost"
          disabled={busy}
          onClick={() => void answer("deny")}
        >
          Skip this run
        </button>
      </div>
      <form
        className="rt-review-reply"
        onSubmit={(event) => {
          event.preventDefault();
          if (text.trim()) void answer("allow", { message: text.trim() });
        }}
      >
        <input
          className="ui-input"
          aria-label="Your answer"
          placeholder="Or write an answer"
          value={text}
          disabled={busy}
          onChange={(event) => setText(event.target.value)}
        />
        <button
          type="submit"
          className="ui-button secondary"
          disabled={busy || !text.trim()}
        >
          Answer
        </button>
      </form>
      {trust && <MetaLine>{trust}</MetaLine>}
      {error && (
        <p role="alert" className="rt-error">
          {error}
        </p>
      )}
    </section>
  );
}

/** R5: the changes a run proposes and holds back until the owner says yes.
 * One checkbox per change; the button applies the checked ones. */
function ApprovalCard({
  host,
  routineId,
  run,
  items,
  when,
  trust,
  onDone,
  onSkip,
  onChange,
}: {
  host: string;
  routineId: string;
  run: number;
  items: PendingAction[];
  when: string;
  trust: string;
  onDone(): void;
  onSkip(): void;
  /** "Change": back to the routine, where it says how it runs. */
  onChange?: () => void;
}) {
  const [picked, setPicked] = useState<number[]>(() =>
    items.map((item) => item.index),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const words = approvalWords(items, picked.length);
  async function approve() {
    setBusy(true);
    setError("");
    try {
      for (const index of picked)
        await routinesClient.approve(host, routineId, run, index);
      onDone();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="rt-review" aria-label="Changes waiting for your OK">
      <Eyebrow>{`${when ? `${when} · ` : ""}${words.heading}`}</Eyebrow>
      <Rows as="ul" label="Changes">
        {items.map((item) => (
          <li key={item.index} className="rt-action">
            <Check
              label={item.title}
              checked={picked.includes(item.index)}
              disabled={busy}
              onChange={(on) =>
                setPicked((current) =>
                  on
                    ? [...current, item.index]
                    : current.filter((index) => index !== item.index),
                )
              }
            />
            <span className="rt-action-text">
              <strong>{item.title}</strong>
              <small>{item.kind}</small>
            </span>
          </li>
        ))}
      </Rows>
      <div className="rt-review-actions">
        <button
          type="button"
          className="ui-button primary"
          disabled={busy || picked.length === 0}
          onClick={() => void approve()}
        >
          {words.button}
        </button>
        <button
          type="button"
          className="ui-button secondary"
          disabled={busy}
          onClick={onSkip}
        >
          Skip this run
        </button>
      </div>
      {trust && (
        <MetaLine>
          {trust}
          {onChange && (
            <>
              {" · "}
              <button type="button" className="rt-link" onClick={onChange}>
                Change
              </button>
            </>
          )}
        </MetaLine>
      )}
      {error && (
        <p role="alert" className="rt-error">
          {error}
        </p>
      )}
    </section>
  );
}

/** The result of one run (R10): the summary, what it did as one card of
 * rows, what is next. */
export function ResultBody({
  result,
  fix = "",
  title = "",
}: {
  result: RunResult;
  /** The fix text on show: a missing tool it already names is not repeated. */
  fix?: string;
  /** Shown in the summary when the document does not start with it. */
  title?: string;
}) {
  const needs = missingNotice(result.missing, fix);
  const counts = actionCounts(result);
  return (
    <div className="rt-result">
      <section aria-label="Summary">
        <Eyebrow>Summary</Eyebrow>
        {title &&
          leadingHeading(result.summary).toLowerCase() !==
            title.trim().toLowerCase() && (
            <h4 className="rt-summary-title">{title}</h4>
          )}
        <DocBody className="rt-summary">
          <MarkdownDoc body={result.summary} />
        </DocBody>
      </section>
      {result.actions.length > 0 && (
        <section aria-label="What it did">
          <Eyebrow
            count={`${counts.applied} applied${counts.wouldDo ? ` · ${counts.wouldDo} would do` : ""}`}
          >
            What it did
          </Eyebrow>
          <Rows as="ul" label="Actions">
            {result.actions.map((action, index) => {
              const link = safeLink(action.link);
              return (
                <li
                  key={index}
                  className={`rt-action${action.applied ? "" : " would"}`}
                >
                  <Dot tone={action.applied ? "ok" : "warning"} />
                  <span className="rt-action-text">
                    <strong>{action.text}</strong>
                    <small>
                      {action.applied
                        ? action.kind
                        : `would do · ${action.kind}`}
                    </small>
                  </span>
                  {link && (
                    <a
                      href={link}
                      aria-label={`Open ${action.kind}`}
                      onClick={(event) => {
                        event.preventDefault();
                        void window.bridge?.agentOpenExternal(link);
                      }}
                    >
                      <ExternalLink size={12} aria-hidden />
                    </a>
                  )}
                </li>
              );
            })}
          </Rows>
        </section>
      )}
      {needs && <MetaLine>{needs}</MetaLine>}
      {result.next && (
        <section aria-label="Next">
          <Eyebrow>Next</Eyebrow>
          <p>{result.next}</p>
        </section>
      )}
    </div>
  );
}

/** The log of a run: the screen as text, kept scrolled to its newest line. */
function RunLog({ text }: { text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    const node = ref.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [text]);
  return (
    <pre className="rt-log" ref={ref}>
      {text}
    </pre>
  );
}

/** R6: what failed, the command that fixes it and the way to try again. A
 * run that stopped for a login offers the terminal; otherwise the command is
 * copied, not run. */
function FailureCard({
  title,
  fix,
  busy,
  login,
  onRunAgain,
}: {
  title: string;
  fix: string;
  busy: boolean;
  login?: Parameters<typeof LoginButton>[0];
  onRunAgain?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const command = fixCommand(fix);
  return (
    <section className="rt-failure" role="alert" aria-label={title}>
      <h3>{title}</h3>
      <DocBody>
        <MarkdownDoc body={fix || "The run ended without an answer."} />
      </DocBody>
      {command && (
        <div className="rt-fix">
          <code>{command}</code>
          <button
            type="button"
            className="ui-button ghost"
            onClick={() => {
              void navigator.clipboard?.writeText(command);
              setCopied(true);
            }}
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      )}
      {(onRunAgain || login) && (
        <div className="rt-review-actions">
          {login && <LoginButton {...login} />}
          {onRunAgain && (
            <button
              type="button"
              className={`ui-button ${login ? "secondary" : "primary"}`}
              disabled={busy}
              onClick={onRunAgain}
            >
              Run again
            </button>
          )}
        </div>
      )}
    </section>
  );
}

/** One run in the detail: header, owner card while it waits (R5), the fix
 * text of a failure (R6), the log of a live run (R7) and the result (R10,
 * R11). `bare` is the rehearsal inside the builder: no header, no tools. */
export function RunView({
  host,
  routineId,
  routineName = "",
  run,
  onBack,
  onRunAgain,
  onResult,
  onAsk,
  onDoc,
  onChanged,
  openSession,
  trust = "",
  head,
  busy = false,
  bare = false,
}: {
  host: string;
  routineId: string;
  routineName?: string;
  run: RunSummary;
  onBack?: () => void;
  onRunAgain?: () => void;
  onResult?: (result: RunResult | null) => void;
  /** "Ask about it": a builder session on the routine. */
  onAsk?: () => void;
  /** "Open as doc": the result as a document page. */
  onDoc?: () => void;
  /** A change of the run was approved: the routine reloads. */
  onChanged?: () => void;
  /** Shows the run's daemon session as a pane. */
  openSession?: CoreViewProps["openSession"];
  /** The routine's own header, drawn instead of the run header while a
   * question of the run is open: the question is the routine's business. */
  head?: ReactNode;
  trust?: string;
  busy?: boolean;
  bare?: boolean;
}) {
  const [result, setResult] = useState<RunResult | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [log, setLog] = useState("");
  const [skipped, setSkipped] = useState(false);
  const sessionHost = sessionHostOf(host);
  const live = run.status === "running";
  const asks = useRunAsks(live ? sessionHost : null, run.session);
  useEffect(() => {
    let current = true;
    setResult(null);
    setError("");
    routinesClient
      .result(host, routineId, run.n)
      .then((value) => {
        if (!current) return;
        setResult(value.result);
        onResult?.(value.result);
      })
      .catch((reason) => {
        if (current)
          setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      current = false;
    };
    // `onResult` is the parent's setter and changes no data.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [host, routineId, run.n, run.status, run.pendingApprovals]);
  useEffect(() => setSkipped(false), [host, routineId, run.n]);
  const readLog = useCallback(async () => {
    if (!run.session || !window.bridge) return;
    try {
      const read = await window.bridge.sessionRead(
        sessionHost,
        run.session,
        80,
      );
      setLog(read.text.trim());
    } catch {
      // An ended session keeps no screen; the result is the record.
    }
  }, [sessionHost, run.session]);
  useEffect(() => {
    void readLog();
    if (!live) return;
    const timer = setInterval(() => void readLog(), 2000);
    return () => clearInterval(timer);
  }, [live, readLog]);

  const failed = run.status === "failed" || run.status === "missing_tool";
  const fix = failureText(run, result);
  const title = result?.title || run.title || `Run ${run.n}`;
  const when = runDateTime(run.startedAt);
  const meta = [
    durationText(run),
    run.agent
      ? [run.agent.harness, run.agent.model].filter(Boolean).join(" ")
      : "",
  ]
    .filter(Boolean)
    .join(" · ");
  const waiting = bare || skipped ? [] : pendingActions(run);
  const session = run.session;
  const login =
    failed && openSession && run.loginHost && run.loginHarness
      ? {
          host,
          loginHost: run.loginHost,
          harness: run.loginHarness,
          openSession,
        }
      : undefined;
  return (
    <section className="rt-run" aria-label={`Run ${run.n}`}>
      {!bare && (asks.length > 0 || waiting.length > 0) && head}
      {!bare && !((asks.length > 0 || waiting.length > 0) && head) && (
        <header className="rt-run-head">
          {onBack && (
            <button type="button" className="rt-back" onClick={onBack}>
              {`← ${routineName || "Back"}`}
            </button>
          )}
          <div className="rt-run-title-row">
            <h2>
              {failed || run.status === "needs_you" || live ? "Run" : "Result"}
              {when ? ` · ${when}` : ` · ${run.n}`}
            </h2>
            {session && openSession && (
              <button
                type="button"
                className="ui-button ghost"
                onClick={() =>
                  openSession(
                    sessionHost,
                    session,
                    `${routineName || "Routine"} · run ${run.n}`,
                  )
                }
              >
                <ExternalLink size={12} aria-hidden />
                Open session
              </button>
            )}
          </div>
          {meta && <MetaLine>{meta}</MetaLine>}
        </header>
      )}
      {asks.map((question) => (
        <ReviewCard
          key={question.askId}
          question={question}
          host={sessionHost}
          trust={trust}
        />
      ))}
      {waiting.length > 0 && (
        <ApprovalCard
          key={waiting.map((item) => item.index).join(",")}
          host={host}
          routineId={routineId}
          run={run.n}
          items={waiting}
          when={when}
          trust={trust}
          onDone={() => onChanged?.()}
          onSkip={() => setSkipped(true)}
          onChange={onBack}
        />
      )}
      {failed && (
        <FailureCard
          title={
            run.status === "failed" ? "This run failed" : "A tool is missing"
          }
          fix={fix}
          busy={busy}
          login={login}
          onRunAgain={onRunAgain}
        />
      )}
      {error && (
        <p role="alert" className="rt-error">
          {error}
        </p>
      )}
      {result && (
        <>
          {!(failed && fix === result.summary && !result.actions.length) && (
            <ResultBody
              result={
                waiting.length > 0
                  ? {
                      ...result,
                      actions: result.actions.filter((item) => item.applied),
                    }
                  : result
              }
              fix={fix}
              title={bare ? "" : title}
            />
          )}
          {!bare && !failed && (
            <div className="rt-detail-actions">
              {onAsk && (
                <button
                  type="button"
                  className="ui-button secondary"
                  onClick={onAsk}
                >
                  Ask about it
                </button>
              )}
              <button
                type="button"
                className="ui-button ghost"
                onClick={() => {
                  void navigator.clipboard?.writeText(result.summary);
                  setCopied(true);
                }}
              >
                {copied ? "Copied" : "Copy"}
              </button>
              {onDoc && (
                <button
                  type="button"
                  className="ui-button ghost"
                  onClick={onDoc}
                >
                  Open as doc
                </button>
              )}
            </div>
          )}
        </>
      )}
      {!result && !error && !live && (
        <MetaLine>This run has no result.</MetaLine>
      )}
      {log && (
        <section aria-label="Run log">
          <Eyebrow>{live ? "Live log" : "Last screen"}</Eyebrow>
          <RunLog text={log} />
        </section>
      )}
    </section>
  );
}
