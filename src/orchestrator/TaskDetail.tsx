import { Fragment, useEffect, useState } from "react";
import {
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  Square,
  Trash2,
} from "lucide-react";
import { orchestratorClient } from "./client";
import {
  attemptDurationMs,
  childrenOf,
  costByStage,
  criteriaMet,
  dependencyTitles,
  fingerprintLabel,
  formatCost,
  formatDuration,
  formatTaskTier,
  implementAttemptCount,
  statusBadgeLabel,
  statusTone,
  totalDurationMs,
  variantLabel,
} from "./helpers";
import type { Attempt, Settings, Task } from "./types";
import { RichText } from "../agents/AgentsView";
import { TaskCostLine, TaskTimelineBar } from "./TaskInsights";

const DECISION_TAGS = ["Jev", "Orchestrator"] as const;

/** Splits a `decisions` line into its automated source (`"Jev: ..."`,
 * `"Orchestrator: ..."`) and the rest of the text, or `null` for anything
 * else (an `"Owner: ..."` line, most often) - the owner's own words need no
 * badge, only the automated decisions do. */
function decisionTag(
  line: string,
): { tag: (typeof DECISION_TAGS)[number]; text: string } | null {
  for (const tag of DECISION_TAGS) {
    const prefix = `${tag}: `;
    if (line.startsWith(prefix))
      return { tag, text: line.slice(prefix.length) };
  }
  return null;
}

/** Same idea as `statusTone`, one level down: a single attempt's own status. */
function attemptTone(status: Attempt["status"]): string {
  switch (status) {
    case "passed":
      return "green";
    case "failed":
      return "red";
    case "blocked":
      return "yellow";
    case "running":
      return "blue";
    default:
      return "muted";
  }
}

/** One verify command's result: exit code plus an expandable tail. */
function VerifyRow({ result }: { result: Attempt["verify"][number] }) {
  const [open, setOpen] = useState(false);
  const failed = result.code !== 0;
  return (
    <div className="orch-verify-row">
      <button
        className="orch-verify-toggle"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <code className="orch-verify-command" title={result.command}>
          {result.command}
        </code>
        <span className={`orch-verify-exit ${failed ? "failed" : ""}`}>
          exit {result.code ?? "—"}
        </span>
      </button>
      {open && result.tail && <pre className="orch-tail">{result.tail}</pre>}
    </div>
  );
}

function AttemptRow({ attempt }: { attempt: Attempt }) {
  const tone = attemptTone(attempt.status);
  return (
    <div className={`orch-attempt orch-attempt-${tone}`}>
      <div className="orch-attempt-head">
        <span className="orch-attempt-n">
          #{attempt.n} · {attempt.stage}
        </span>
        <span className="orch-attempt-route">
          {attempt.harness} ·{" "}
          {attempt.fingerprint
            ? fingerprintLabel(attempt.fingerprint)
            : attempt.model || attempt.routeId}
          <span className={`orch-attempt-inline-status tone-${tone}`}>
            {" "}
            {attempt.status === "running" ? "running…" : attempt.status}
          </span>
        </span>
      </div>
      <p className="orch-attempt-reason">{attempt.reason}</p>
      <p className="orch-attempt-meta">
        {formatDuration(attemptDurationMs(attempt))} ·{" "}
        {formatCost(attempt.costUsd)} · {attempt.changedFiles.length} file
        {attempt.changedFiles.length === 1 ? "" : "s"} changed
        {attempt.gateBlocks > 0 &&
          ` · ${attempt.gateBlocks} Stop-hook block${attempt.gateBlocks === 1 ? "" : "s"}`}
      </p>
      {attempt.verify.map((result, index) => (
        <VerifyRow key={index} result={result} />
      ))}
      {attempt.review && (
        <p
          className={`orch-review orch-review-${attempt.review.verdict.toLowerCase()}`}
        >
          Review: {attempt.review.verdict}
          {attempt.review.findings.length > 0 &&
            ` — ${attempt.review.findings.join("; ")}`}
        </p>
      )}
      {attempt.failure && (
        <p className="orch-failure">{attempt.failure.detail}</p>
      )}
      {attempt.summary && <p className="orch-summary">{attempt.summary}</p>}
    </div>
  );
}

/** The images the latest passing implement attempt saved, as thumbnails that
 * open full size. Orchd serves them as data URLs (the renderer cannot read
 * its data directory). */
function EvidenceGallery({ task }: { task: Task }) {
  const latest = [...task.attempts]
    .reverse()
    .find(
      (a) =>
        a.stage === "implement" &&
        a.status === "passed" &&
        (a.evidence?.length ?? 0) > 0,
    );
  const paths = latest?.evidence ?? [];
  const key = paths.join("\n");
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setUrls({});
    for (const path of key ? key.split("\n") : []) {
      orchestratorClient
        .taskEvidence(task.id, path)
        .then(({ dataUrl }) => {
          if (!cancelled) setUrls((prev) => ({ ...prev, [path]: dataUrl }));
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
    };
  }, [task.id, key]);
  if (paths.length === 0) return null;
  const name = (path: string) => path.split("/").pop() ?? path;
  return (
    <div className="orch-evidence">
      <span className="dialog-eyebrow">EVIDENCE</span>
      <div className="orch-evidence-grid">
        {paths.map((path) =>
          urls[path] ? (
            <button
              key={path}
              className="orch-evidence-thumb"
              title={name(path)}
              aria-label={`Open ${name(path)}`}
              onClick={() => setOpen(path)}
            >
              <img src={urls[path]} alt={name(path)} />
              <span>{name(path)}</span>
            </button>
          ) : null,
        )}
      </div>
      {open && urls[open] && (
        <div
          className="orch-evidence-full"
          role="dialog"
          aria-label={name(open)}
          onClick={() => setOpen(null)}
        >
          <img src={urls[open]} alt={name(open)} />
        </div>
      )}
    </div>
  );
}

/** An upcoming stage that hasn't run yet - review, most often - shown dimmed
 * on the rail so the rail reads as "what will happen", not just history. */
function PendingStageRow({ label }: { label: string }) {
  return (
    <div className="orch-attempt orch-attempt-pending orch-attempt-muted">
      <div className="orch-attempt-head">
        <span className="orch-attempt-n">review</span>
        <span className="orch-attempt-route">
          {label}
          <span className="orch-attempt-inline-status tone-muted">
            {" "}
            pending
          </span>
        </span>
      </div>
    </div>
  );
}

function QuestionCard({
  task,
  disabled,
  onAnswer,
}: {
  task: Task;
  disabled: boolean;
  onAnswer(answer: string): void;
}) {
  const [freeText, setFreeText] = useState("");
  if (!task.question) return null;
  return (
    <div className="orch-question">
      <p className="orch-question-text">{task.question.text}</p>
      <div className="orch-question-options">
        {task.question.options.map((option) => (
          <button
            key={option}
            className="secondary"
            disabled={disabled}
            onClick={() => onAnswer(option)}
          >
            {option}
          </button>
        ))}
      </div>
      <form
        className="orch-question-free"
        onSubmit={(event) => {
          event.preventDefault();
          if (freeText.trim()) {
            onAnswer(freeText.trim());
            setFreeText("");
          }
        }}
      >
        <input
          value={freeText}
          onChange={(event) => setFreeText(event.target.value)}
          placeholder="Your own answer…"
          aria-label="Your own answer"
        />
        <button
          className="primary"
          type="submit"
          disabled={disabled || !freeText.trim()}
        >
          Answer
        </button>
      </form>
    </div>
  );
}

/** The finished task's report at the top of its detail, with the lead-touch
 * toggle: whether the work needed a fix after orchd said done. */
function TaskReport({
  task,
  tasks,
  disabled,
  onOpen,
  onLeadTouch,
}: {
  task: Task;
  tasks: Task[];
  disabled: boolean;
  onOpen(id: string): void;
  onLeadTouch(touched: boolean | undefined, note?: string): void;
}) {
  const mark = task.leadTouch;
  const [noting, setNoting] = useState(false);
  const [note, setNote] = useState("");
  return (
    <section className="orch-report" aria-label="Report">
      <div className="orch-report-head">
        <span className="dialog-eyebrow">REPORT</span>
        <span className="orch-report-touch">
          {mark
            ? `${mark.touched ? "Needed a fix" : "Clean"} (${mark.by})${mark.note ? `: ${mark.note}` : ""}`
            : "Lead touch unknown"}
        </span>
        <button
          className={mark?.touched === true ? "primary" : "secondary"}
          disabled={disabled}
          aria-pressed={mark?.touched === true}
          onClick={() => {
            if (mark?.touched === true) onLeadTouch(undefined);
            else setNoting(true);
          }}
        >
          Needed a fix
        </button>
        <button
          className={mark?.touched === false ? "primary" : "secondary"}
          disabled={disabled}
          aria-pressed={mark?.touched === false}
          onClick={() =>
            onLeadTouch(mark?.touched === false ? undefined : false)
          }
        >
          Clean
        </button>
      </div>
      {noting && (
        <form
          className="orch-question-free"
          onSubmit={(event) => {
            event.preventDefault();
            onLeadTouch(true, note.trim());
            setNoting(false);
            setNote("");
          }}
        >
          <input
            value={note}
            onChange={(event) => setNote(event.target.value)}
            placeholder="What is missing or wrong? A follow-up task will be created."
            aria-label="What had to be fixed"
            autoFocus
          />
          <button className="primary" type="submit" disabled={disabled}>
            Save
          </button>
          <button
            className="secondary"
            type="button"
            onClick={() => setNoting(false)}
          >
            Cancel
          </button>
        </form>
      )}
      {(task.followUps ?? []).length > 0 && (
        <p className="orch-detail-meta">
          {(task.followUps ?? []).map((id, i) => (
            <Fragment key={id}>
              {i > 0 && " · "}
              <button className="orch-parent-link" onClick={() => onOpen(id)}>
                Follow-up:{" "}
                {tasks.find((t) => t.id === id)?.title ?? id.slice(0, 8)}
              </button>
            </Fragment>
          ))}
        </p>
      )}
      {task.report && (
        <div className="orch-report-body">
          <RichText text={task.report} />
        </div>
      )}
    </section>
  );
}

/** The planner's own answers to its non-blocking questions, each with an
 * Overturn action: the owner's text replaces the assumption and reaches the
 * task's next attempt. */
function AssumptionsList({
  task,
  disabled,
  onOverturn,
}: {
  task: Task;
  disabled: boolean;
  onOverturn(index: number, answer: string): void;
}) {
  const [editing, setEditing] = useState<number | null>(null);
  const [text, setText] = useState("");
  const assumptions = task.assumptions ?? [];
  if (assumptions.length === 0) return null;
  return (
    <div className="orch-assumptions">
      <span className="dialog-eyebrow">ASSUMPTIONS</span>
      {assumptions.map((assumption, index) => (
        <div key={index} className="orch-assumption">
          <p className="orch-assumption-question">{assumption.question}</p>
          <p className="orch-assumption-answer">
            <span className="orch-decision-tag orch-decision-jev">
              {assumption.by}
            </span>
            {assumption.answer}
            {assumption.overturned && (
              <span className="orch-assumption-overturned">
                {" "}
                - overturned: {assumption.ownerAnswer}
              </span>
            )}
          </p>
          {assumption.evidence && (
            <p className="orch-assumption-evidence">{assumption.evidence}</p>
          )}
          {editing === index ? (
            <form
              className="orch-question-free"
              onSubmit={(event) => {
                event.preventDefault();
                if (!text.trim()) return;
                onOverturn(index, text.trim());
                setEditing(null);
                setText("");
              }}
            >
              <input
                value={text}
                onChange={(event) => setText(event.target.value)}
                placeholder="Your answer instead…"
                aria-label="Your answer instead"
                autoFocus
              />
              <button
                className="primary"
                type="submit"
                disabled={disabled || !text.trim()}
              >
                Send
              </button>
              <button
                className="secondary"
                type="button"
                onClick={() => setEditing(null)}
              >
                Cancel
              </button>
            </form>
          ) : (
            <button
              className="secondary"
              disabled={disabled}
              onClick={() => {
                setEditing(index);
                setText("");
              }}
            >
              {assumption.overturned ? "Overturn again" : "Overturn"}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

/** One task's view: header and actions, the question blocking it, its report,
 * timeline, criteria, subtasks, assumptions, decisions, evidence, attempts
 * and live log. */
export function TaskDetail({
  selected,
  tasks,
  settings,
  busy,
  logLines,
  act,
  onOpen,
  onDelete,
}: {
  selected: Task;
  /** Every task of the repo, archived included - for parents, children,
   * dependencies and follow-ups. */
  tasks: Task[];
  settings: Settings | null;
  busy: boolean;
  logLines: string[] | undefined;
  act(action: () => Promise<Task>): void;
  onOpen(id: string): void;
  onDelete(): void;
}) {
  const children = childrenOf(tasks, selected.id);
  const parentTask = selected.parent
    ? tasks.find((t) => t.id === selected.parent)
    : undefined;
  const waitsFor = dependencyTitles(selected, tasks);
  return (
    <div className="orch-detail">
      <div className="orch-detail-head">
        <div className="orch-detail-title">
          <h3>{selected.title}</h3>
          <p className="orch-detail-meta">
            attempt {implementAttemptCount(selected)}/
            {settings?.maxAttempts ?? implementAttemptCount(selected)} ·{" "}
            {formatDuration(totalDurationMs(selected))} ·{" "}
            <span title="API list price the CLI reports, cache included; a subscription is not billed per token">
              {formatCost(selected.costUsd)}
            </span>{" "}
            ·{" "}
            <span
              className={
                selected.tierFallback ? "orch-tier-fallback" : undefined
              }
            >
              {formatTaskTier(selected)}
            </span>{" "}
            ·{" "}
            <span className="orch-branch" title={selected.branch}>
              {selected.branch}
            </span>
            {selected.baseRef && (
              <>
                {" "}
                <span className="orch-branch" title={selected.baseRef}>
                  from {selected.baseRef}
                </span>
              </>
            )}
            {selected.variant?.land && (
              <>
                {" · "}
                <span className="orch-branch" title={selected.landedSha}>
                  {selected.landedSha
                    ? `landed ${selected.landedSha.slice(0, 8)}`
                    : selected.status === "landing"
                      ? "landing: waiting for a clean checkout"
                      : "lands on its base"}
                </span>
              </>
            )}
          </p>
          {(parentTask || waitsFor.length > 0 || selected.queueReason) && (
            <p className="orch-detail-meta">
              {parentTask && (
                <button
                  className="orch-parent-link"
                  onClick={() => onOpen(parentTask.id)}
                >
                  Part of {parentTask.title}
                </button>
              )}
              {parentTask && waitsFor.length > 0 && " · "}
              {waitsFor.length > 0 && `after ${waitsFor.join(", ")}`}
              {selected.queueReason &&
                `${parentTask || waitsFor.length > 0 ? " · " : ""}${selected.queueReason}`}
            </p>
          )}
          {settings && (
            <p className="orch-detail-meta orch-variant-line">
              Variant: {variantLabel(selected, settings.experiments)}
            </p>
          )}
          {(() => {
            const breakdown = costByStage(selected);
            // A parent's total includes its subtasks' costs.
            const subtasks = children.reduce(
              (sum, child) => sum + child.costUsd,
              0,
            );
            const legacy: { label: string; cost: number }[] = [
              { label: "Plan", cost: breakdown.plan },
              ...breakdown.implement.map((a) => ({
                label: `Implement #${a.n}`,
                cost: a.costUsd,
              })),
              { label: "Review", cost: breakdown.review },
              { label: "Subtasks", cost: subtasks },
              {
                label: "Other",
                cost: Math.max(0, breakdown.other - subtasks),
              },
            ].filter((row) => formatCost(row.cost) !== "$0.00");
            return (
              <TaskCostLine
                task={selected}
                subtasks={subtasks}
                legacy={legacy}
              />
            );
          })()}
        </div>
        <div className="orch-detail-actions">
          {(selected.status === "drafting" ||
            selected.status === "queued" ||
            selected.status === "stopped" ||
            selected.status === "failed") && (
            <button
              // A stopped or failed task is idle, waiting on you -
              // restarting it is the one action that matters, so it
              // gets the filled/primary treatment.
              className={
                selected.status === "stopped" || selected.status === "failed"
                  ? "primary"
                  : "secondary"
              }
              disabled={busy}
              onClick={() =>
                act(() => orchestratorClient.taskStart(selected.id))
              }
            >
              {selected.status === "drafting" || selected.status === "queued"
                ? "Start"
                : "Run again"}
            </button>
          )}
          {selected.status === "done" &&
            !selected.parent &&
            !selected.landedSha &&
            !!selected.baseRef && (
              <button
                disabled={busy}
                onClick={() =>
                  act(() => orchestratorClient.taskLand(selected.id))
                }
              >
                Land
              </button>
            )}
          {(selected.status === "drafting" ||
            selected.status === "running" ||
            selected.status === "queued" ||
            selected.status === "waiting") && (
            <button
              className="secondary"
              disabled={busy}
              onClick={() =>
                act(() => orchestratorClient.taskStop(selected.id))
              }
            >
              <Square size={12} /> Stop
            </button>
          )}
          <button
            className="icon-button orch-archive-button"
            title="Archive task"
            disabled={
              busy ||
              selected.status === "running" ||
              selected.status === "drafting" ||
              selected.status === "waiting"
            }
            onClick={() =>
              act(() => orchestratorClient.taskArchive(selected.id))
            }
          >
            <Archive size={13} />
          </button>
          <button
            className="icon-button orch-delete-button"
            title="Delete task"
            disabled={busy}
            onClick={onDelete}
          >
            <Trash2 size={13} />
          </button>
        </div>
      </div>
      {/* The one thing blocking the task goes first, above everything
       * that only describes it. */}
      {selected.question && (
        <QuestionCard
          key={`question-${selected.id}`}
          task={selected}
          disabled={busy}
          onAnswer={(answer) =>
            act(() => orchestratorClient.taskAnswer(selected.id, answer))
          }
        />
      )}
      {selected.status === "done" && (
        <TaskReport
          key={`report-${selected.id}`}
          task={selected}
          tasks={tasks}
          disabled={busy}
          onOpen={onOpen}
          onLeadTouch={(touched, note) =>
            act(() =>
              orchestratorClient.taskLeadTouch(selected.id, touched, note),
            )
          }
        />
      )}
      <TaskTimelineBar task={selected} />
      {selected.request && selected.request !== selected.title && (
        <p className="orch-request">{selected.request}</p>
      )}
      {selected.criteria.length > 0 && (
        <div className="orch-criteria">
          {selected.criteria.map((criterion, index) => (
            <p key={index}>
              {criteriaMet(selected) ? (
                <Check size={12} className="orch-check-met" />
              ) : (
                <Square size={12} className="orch-check-open" />
              )}{" "}
              {criterion}
            </p>
          ))}
        </div>
      )}
      {children.length > 0 && (
        <div className="orch-subtasks">
          <span className="dialog-eyebrow">SUBTASKS</span>
          {children.map((child) => {
            const tone = statusTone(child);
            const after = dependencyTitles(child, tasks);
            return (
              <button
                key={child.id}
                className="orch-subtask-row"
                onClick={() => onOpen(child.id)}
              >
                <span className={`status-dot ${tone}`} />
                <span className="orch-task-title">{child.title}</span>
                {after.length > 0 && (
                  <span className="orch-subtask-after">
                    after {after.join(", ")}
                  </span>
                )}
                {child.queueReason && (
                  <span className="orch-subtask-after">
                    {child.queueReason}
                  </span>
                )}
                <span className={`orch-status-badge tone-${tone}`}>
                  {statusBadgeLabel(child)}
                </span>
              </button>
            );
          })}
        </div>
      )}
      <AssumptionsList
        key={`assumptions-${selected.id}`}
        task={selected}
        disabled={busy}
        onOverturn={(index, answer) =>
          act(() => orchestratorClient.taskOverturn(selected.id, index, answer))
        }
      />
      {selected.decisions.length > 0 && (
        <div className="orch-decisions">
          {selected.decisions.map((line, index) => {
            const parsed = decisionTag(line);
            return (
              <p key={index} className="orch-decision-line">
                {parsed && (
                  <span
                    className={`orch-decision-tag orch-decision-${parsed.tag.toLowerCase()}`}
                  >
                    {parsed.tag}
                  </span>
                )}
                {parsed ? parsed.text : line}
              </p>
            );
          })}
        </div>
      )}
      <EvidenceGallery task={selected} />
      <div className="orch-attempts">
        {selected.attempts.map((attempt) => (
          <AttemptRow key={attempt.n} attempt={attempt} />
        ))}
        {settings?.review &&
          children.length === 0 &&
          !selected.attempts.some((a) => a.stage === "review") &&
          (selected.status === "running" || selected.status === "queued") && (
            <PendingStageRow
              label={
                settings.review === "auto"
                  ? "other vendor"
                  : (settings.routes.find((r) => r.id === settings.review)
                      ?.label ?? settings.review)
              }
            />
          )}
      </div>
      {logLines && logLines.length > 0 && (
        <pre className="orch-log">{logLines.join("\n")}</pre>
      )}
    </div>
  );
}
