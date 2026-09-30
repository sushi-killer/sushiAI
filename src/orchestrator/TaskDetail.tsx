import { Fragment, useEffect, useState } from "react";
import {
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  GitBranch,
  Square,
  Trash2,
} from "lucide-react";
import { useOrchestratorClient } from "./hostContext";
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
  stageTrack,
  statusBadgeLabel,
  statusTone,
  type Tone,
  variantLabel,
} from "./helpers";
import type { Attempt, Settings, Task } from "./types";
import { RichText } from "../agents/AgentsView";
import { TaskCostLine, TaskTimeline } from "./TaskInsights";
import {
  acceptanceHeading,
  followUpLabel,
  headerFacts,
  questionSource,
  reportBody,
  reportLine,
  reportSummary,
  shortBranch,
} from "./taskDetailModel";
import { Chip, Criterion, StageTrack, Tag } from "./ui";
import "./task-detail.css";

const DECISION_TAGS = ["Orchestrator"] as const;

/** Splits an `Orchestrator: ...` decisions line and the rest of the text,
 * or `null` for anything
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

/** A single attempt's own status as a tag tone. */
function attemptTone(status: Attempt["status"]): Tone {
  switch (status) {
    case "passed":
      return "ok";
    case "failed":
      return "danger";
    case "blocked":
      return "warning";
    case "running":
      return "info";
    default:
      return "neutral";
  }
}

/** One verify command's result: exit code plus an expandable tail. */
function VerifyRow({ result }: { result: Attempt["verify"][number] }) {
  const [open, setOpen] = useState(false);
  const failed = result.code !== 0;
  return (
    <div className="td-verify-row">
      <button
        className="td-verify-toggle"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <code className="td-verify-command" title={result.command}>
          {result.command}
        </code>
        <span className={`td-verify-exit ${failed ? "failed" : ""}`}>
          exit {result.code ?? "—"}
        </span>
      </button>
      {open && result.tail && <pre className="td-tail">{result.tail}</pre>}
    </div>
  );
}

function AttemptRow({ attempt }: { attempt: Attempt }) {
  const tone = attemptTone(attempt.status);
  const files = attempt.changedFiles.length;
  return (
    <div className="td-attempt">
      <span className="td-attempt-rail">
        <span className={`td-dot ui-tone-${tone}`} />
      </span>
      <div className="td-attempt-body">
        <div className="td-attempt-head">
          <span className="td-attempt-title">
            #{attempt.n} · {attempt.stage}
          </span>
          <span className="td-attempt-route">
            {attempt.harness} ·{" "}
            {attempt.fingerprint
              ? fingerprintLabel(attempt.fingerprint)
              : attempt.model || attempt.routeId}
          </span>
          <Tag tone={tone}>
            {attempt.status === "running" ? "running" : attempt.status}
          </Tag>
        </div>
        <p className="td-attempt-meta">
          {formatDuration(attemptDurationMs(attempt))} ·{" "}
          {formatCost(attempt.costUsd)} · {files} file{files === 1 ? "" : "s"}{" "}
          changed
          {attempt.gateBlocks > 0 &&
            ` · ${attempt.gateBlocks} Stop-hook block${attempt.gateBlocks === 1 ? "" : "s"}`}
        </p>
        {/* The plain tier pick lives under Details; only a changed route
         * (an escalation, a retry on another vendor) is news here. */}
        {attempt.reason && attempt.reason !== "tier default" && (
          <p className="td-attempt-meta">{attempt.reason}</p>
        )}
        {attempt.verify.map((result, index) => (
          <VerifyRow key={index} result={result} />
        ))}
        {attempt.review && (
          <p
            className={`td-attempt-note ${attempt.review.verdict === "FAIL" ? "danger" : ""}`}
          >
            Review: {attempt.review.verdict}
            {attempt.review.findings.length > 0 &&
              ` — ${attempt.review.findings.join("; ")}`}
          </p>
        )}
        {attempt.failure && (
          <p className="td-attempt-note danger">{attempt.failure.detail}</p>
        )}
        {attempt.summary && (
          <p className="td-attempt-note">{attempt.summary}</p>
        )}
      </div>
    </div>
  );
}

/** The images the latest passing implement attempt saved, as thumbnails that
 * open full size. Orchd serves them as data URLs (the renderer cannot read
 * its data directory). */
function EvidenceGallery({ task }: { task: Task }) {
  const orchestratorClient = useOrchestratorClient();
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
  }, [task.id, key, orchestratorClient]);
  if (paths.length === 0) return null;
  const name = (path: string) => path.split("/").pop() ?? path;
  return (
    <div className="td-evidence">
      <span className="td-eyebrow">EVIDENCE</span>
      <div className="td-evidence-row">
        {paths.map((path) =>
          urls[path] ? (
            <button
              key={path}
              className="td-thumb"
              title={name(path)}
              aria-label={`Open ${name(path)}`}
              onClick={() => setOpen(path)}
            >
              <span className="td-thumb-image">
                <img src={urls[path]} alt={name(path)} />
              </span>
              <span className="td-thumb-caption">{name(path)}</span>
            </button>
          ) : null,
        )}
      </div>
      {open && urls[open] && (
        <div
          className="td-evidence-full"
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

/** Files the task named that are not in its commit, each with where it was
 * kept and an Open action. */
function Deliverables({ task }: { task: Task }) {
  const orchestratorClient = useOrchestratorClient();
  const [failed, setFailed] = useState<string | null>(null);
  const list = task.deliverables ?? [];
  if (list.length === 0) return null;
  return (
    <div className="td-evidence">
      <span className="td-eyebrow">DELIVERABLES</span>
      {list.map((d) => (
        <div key={d.path} className="td-deliverable">
          <span className="td-deliverable-path" title={d.placed ?? d.saved}>
            {d.path}
          </span>
          <span className="td-deliverable-where">
            {d.placed ? "in the repo and the task data" : "in the task data"}
          </span>
          <button
            className="td-link"
            aria-label={`Open ${d.path}`}
            onClick={() =>
              orchestratorClient
                .taskOpenDeliverable(task.id, d.path)
                .then(() => setFailed(null))
                .catch(() => setFailed(d.path))
            }
          >
            Open
          </button>
          {failed === d.path && <span>Could not open it</span>}
        </div>
      ))}
    </div>
  );
}

/** An upcoming stage that hasn't run yet - review, most often - shown dimmed
 * so the list reads as "what will happen", not just history. */
function PendingStageRow({ label }: { label: string }) {
  return (
    <div className="td-attempt pending">
      <span className="td-attempt-rail">
        <span className="td-dot ui-tone-neutral" />
      </span>
      <div className="td-attempt-body">
        <div className="td-attempt-head">
          <span className="td-attempt-title">review</span>
          <span className="td-attempt-route">{label}</span>
          <Tag tone="neutral">pending</Tag>
        </div>
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
  const options = task.question?.options ?? [];
  // The first option is picked up front: one click on Answer sends it.
  const [picked, setPicked] = useState(options[0] ?? "");
  const [freeText, setFreeText] = useState("");
  if (!task.question) return null;
  const answer = freeText.trim() || (options.includes(picked) ? picked : "");
  const source = questionSource(task.question);
  return (
    <div className="td-question">
      <div className="td-question-head">
        <Tag tone="warning">Needs you</Tag>
        {source && <span className="td-question-source">{source}</span>}
      </div>
      <p className="td-question-text">{task.question.text}</p>
      {options.length > 0 && (
        <div className="td-question-options">
          {options.map((option) => (
            <Chip
              key={option}
              selected={!freeText.trim() && option === picked}
              disabled={disabled}
              onClick={() => {
                setPicked(option);
                setFreeText("");
              }}
            >
              {option}
            </Chip>
          ))}
        </div>
      )}
      <form
        className="td-inline-form"
        onSubmit={(event) => {
          event.preventDefault();
          if (!answer) return;
          onAnswer(answer);
          setFreeText("");
        }}
      >
        <input
          value={freeText}
          onChange={(event) => setFreeText(event.target.value)}
          placeholder="Your own answer…"
          aria-label="Your own answer"
        />
        <button
          className="ui-button primary"
          type="submit"
          disabled={disabled || !answer}
        >
          Answer
        </button>
      </form>
    </div>
  );
}

/** The finished task's report at the top of its detail: the facts line, who
 * marked the lead touch, follow-ups and the written report. */
function TaskReport({
  task,
  tasks,
  onOpen,
}: {
  task: Task;
  tasks: Task[];
  onOpen(id: string): void;
}) {
  const mark = task.leadTouch;
  const facts = reportLine(task);
  const followUps = task.followUps ?? [];
  const [expanded, setExpanded] = useState(false);
  const summary = task.report ? reportSummary(task.report, task.title) : "";
  if (!task.report && !facts && !mark && followUps.length === 0) return null;
  return (
    <section className="td-report" aria-label="Report">
      <div className="td-report-head">
        <span className="td-eyebrow">REPORT</span>
        {facts && <span className="td-faint">{facts}</span>}
        {mark && (
          <span className="td-faint">
            {mark.touched ? "Needed a fix" : "Clean"} ({mark.by})
            {mark.note ? `: ${mark.note}` : ""}
          </span>
        )}
      </div>
      {followUps.length > 0 && (
        <p className="td-facts">
          {followUps.map((id, i) => (
            <Fragment key={id}>
              {i > 0 && " · "}
              <button className="td-link" onClick={() => onOpen(id)}>
                {followUpLabel(
                  tasks.find((t) => t.id === id)?.title ?? id.slice(0, 8),
                )}
              </button>
            </Fragment>
          ))}
        </p>
      )}
      {expanded && task.report ? (
        <div className="td-report-body">
          <RichText text={reportBody(task.report)} />
        </div>
      ) : (
        summary && <p className="td-report-summary">{summary}</p>
      )}
      {task.report && (
        <button
          className="td-link td-report-toggle"
          aria-expanded={expanded}
          onClick={() => setExpanded((open) => !open)}
        >
          {expanded ? "Hide full report" : "Show full report"}
        </button>
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
    <div className="td-assumptions">
      <span className="td-eyebrow">ASSUMPTIONS</span>
      {assumptions.map((assumption, index) => (
        <div key={index} className="td-assumption">
          <p className="td-assumption-question">{assumption.question}</p>
          <p className="td-assumption-answer">
            <span className="td-decision-tag">{assumption.by}</span>
            {assumption.answer}
            {assumption.overturned && (
              <span className="td-assumption-overturned">
                {" "}
                - overturned: {assumption.ownerAnswer}
              </span>
            )}
          </p>
          {assumption.evidence && (
            <p className="td-assumption-evidence">{assumption.evidence}</p>
          )}
          {editing === index ? (
            <form
              className="td-inline-form"
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
                className="ui-button primary"
                type="submit"
                disabled={disabled || !text.trim()}
              >
                Send
              </button>
              <button
                className="ui-button secondary"
                type="button"
                onClick={() => setEditing(null)}
              >
                Cancel
              </button>
            </form>
          ) : (
            <button
              className="ui-button secondary"
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

/** One task's view: header and actions, the question blocking it, its report
 * or stage track, timeline, criteria, attempts and evidence, and the rest
 * (request, subtasks, assumptions, decisions, live log) under Details. */
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
  const orchestratorClient = useOrchestratorClient();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [noting, setNoting] = useState(false);
  const [note, setNote] = useState("");
  const children = childrenOf(tasks, selected.id);
  const parentTask = selected.parent
    ? tasks.find((t) => t.id === selected.parent)
    : undefined;
  const waitsFor = dependencyTitles(selected, tasks);
  const done = selected.status === "done";
  const mark = selected.leadTouch;
  const statusTag: { tone: Tone; label: string } | null = done
    ? { tone: "ok", label: selected.landedSha ? "landed" : "done" }
    : selected.status === "failed" || selected.status === "stopped"
      ? { tone: "danger", label: statusBadgeLabel(selected) }
      : selected.status === "landing"
        ? { tone: "warning", label: "landing" }
        : null;
  const met = criteriaMet(selected);
  const breakdown = costByStage(selected);
  // A parent's total includes its subtasks' costs.
  const subtasksCost = children.reduce((sum, child) => sum + child.costUsd, 0);
  const legacy: { label: string; cost: number }[] = [
    { label: "Plan", cost: breakdown.plan },
    ...breakdown.implement.map((a) => ({
      label: `Implement #${a.n}`,
      cost: a.costUsd,
    })),
    { label: "Review", cost: breakdown.review },
    { label: "Subtasks", cost: subtasksCost },
    { label: "Other", cost: Math.max(0, breakdown.other - subtasksCost) },
  ].filter((row) => formatCost(row.cost) !== "$0.00");
  const hasDetails =
    (selected.request && selected.request !== selected.title) ||
    children.length > 0 ||
    (selected.assumptions ?? []).length > 0 ||
    selected.decisions.length > 0 ||
    !!settings ||
    (logLines?.length ?? 0) > 0;
  return (
    <div className="td">
      <div className="td-scroll">
        <div className="td-head">
          <div className="td-title">
            <div className="td-title-row">
              <h3 title={selected.title}>{selected.title}</h3>
              {statusTag && <Tag tone={statusTag.tone}>{statusTag.label}</Tag>}
            </div>
            <p className="td-meta">
              <GitBranch size={12} aria-hidden />
              <span className="td-branch" title={selected.branch}>
                {shortBranch(selected.branch)}
              </span>
              {headerFacts(selected, settings?.maxAttempts).map((fact) => (
                <span key={fact} className="td-faint td-fact">
                  {fact}
                </span>
              ))}
            </p>
          </div>
          <div className="td-actions">
            {(selected.status === "drafting" ||
              selected.status === "queued" ||
              selected.status === "stopped" ||
              selected.status === "failed") && (
              <button
                // A stopped or failed task is idle, waiting on you -
                // restarting it is the one action that matters.
                className={`ui-button ${
                  selected.status === "stopped" || selected.status === "failed"
                    ? "primary"
                    : "secondary"
                }`}
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
            {done &&
              !selected.parent &&
              !selected.landedSha &&
              !!selected.baseRef && (
                <button
                  className="ui-button secondary"
                  disabled={busy}
                  onClick={() =>
                    act(() => orchestratorClient.taskLand(selected.id))
                  }
                >
                  Land
                </button>
              )}
            {done && (
              <>
                <button
                  className="ui-button ghost"
                  disabled={busy}
                  aria-pressed={mark?.touched === true}
                  onClick={() => {
                    if (mark?.touched === true)
                      act(() =>
                        orchestratorClient.taskLeadTouch(
                          selected.id,
                          undefined,
                        ),
                      );
                    else setNoting(true);
                  }}
                >
                  Needed a fix
                </button>
                <button
                  className="ui-button secondary"
                  disabled={busy}
                  aria-pressed={mark?.touched === false}
                  onClick={() =>
                    act(() =>
                      orchestratorClient.taskLeadTouch(
                        selected.id,
                        mark?.touched === false ? undefined : false,
                      ),
                    )
                  }
                >
                  <Check size={14} />
                  Clean
                </button>
              </>
            )}
            {(selected.status === "drafting" ||
              selected.status === "running" ||
              selected.status === "queued" ||
              selected.status === "waiting") && (
              <button
                className="ui-button secondary"
                disabled={busy}
                onClick={() =>
                  act(() => orchestratorClient.taskStop(selected.id))
                }
              >
                <Square size={12} /> Stop
              </button>
            )}
            {/* Archive for a task at rest, delete for one not finished. */}
            {selected.status !== "running" &&
              selected.status !== "drafting" &&
              selected.status !== "waiting" && (
                <button
                  className="td-icon-button"
                  title="Archive task"
                  aria-label="Archive task"
                  disabled={busy}
                  onClick={() =>
                    act(() => orchestratorClient.taskArchive(selected.id))
                  }
                >
                  <Archive size={16} />
                </button>
              )}
            {!done && (
              <button
                className="td-icon-button"
                title="Delete task"
                aria-label="Delete task"
                disabled={busy}
                onClick={onDelete}
              >
                <Trash2 size={16} />
              </button>
            )}
          </div>
        </div>
        {noting && (
          <form
            className="td-inline-form"
            onSubmit={(event) => {
              event.preventDefault();
              act(() =>
                orchestratorClient.taskLeadTouch(
                  selected.id,
                  true,
                  note.trim(),
                ),
              );
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
            <button className="ui-button primary" type="submit" disabled={busy}>
              Save
            </button>
            <button
              className="ui-button secondary"
              type="button"
              onClick={() => setNoting(false)}
            >
              Cancel
            </button>
          </form>
        )}
        {(parentTask || waitsFor.length > 0 || selected.queueReason) && (
          <p className="td-facts">
            {parentTask && (
              <button className="td-link" onClick={() => onOpen(parentTask.id)}>
                Part of {parentTask.title}
              </button>
            )}
            {parentTask && waitsFor.length > 0 && " · "}
            {waitsFor.length > 0 && `after ${waitsFor.join(", ")}`}
            {selected.queueReason &&
              `${parentTask || waitsFor.length > 0 ? " · " : ""}${selected.queueReason}`}
          </p>
        )}
        {done ? (
          <TaskReport
            key={`report-${selected.id}`}
            task={selected}
            tasks={tasks}
            onOpen={onOpen}
          />
        ) : (
          <StageTrack steps={stageTrack(selected)} />
        )}
        {/* The one thing blocking the task goes first, above everything
         * that only describes it. */}
        {selected.question && (
          <QuestionCard
            key={`question-${selected.id}-${selected.question.text}`}
            task={selected}
            disabled={busy}
            onAnswer={(answer) =>
              act(() => orchestratorClient.taskAnswer(selected.id, answer))
            }
          />
        )}
        {/* A finished task reads report then timeline; a live one reads
         * what is asked and checked first. */}
        {done && <TaskTimeline task={selected} />}
        {selected.criteria.length > 0 && (
          <div className="td-criteria">
            <span className="td-eyebrow">
              {acceptanceHeading(selected, met)}
            </span>
            {selected.criteria.map((criterion, index) => (
              <Criterion key={index} state={met ? "met" : "pending"}>
                {criterion}
              </Criterion>
            ))}
          </div>
        )}
        {(selected.attempts.length > 0 || !!settings?.review) && (
          <div className="td-attempts">
            {selected.attempts.length > 0 && (
              <span className="td-eyebrow">ATTEMPTS</span>
            )}
            {selected.attempts.map((attempt) => (
              <AttemptRow key={attempt.n} attempt={attempt} />
            ))}
            {settings?.review &&
              children.length === 0 &&
              !selected.attempts.some((a) => a.stage === "review") &&
              (selected.status === "running" ||
                selected.status === "queued") && (
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
        )}
        {!done && <TaskTimeline task={selected} />}
        <EvidenceGallery task={selected} />
        <Deliverables task={selected} />
        {hasDetails && (
          <section className="td-details">
            <button
              className="td-details-toggle"
              aria-expanded={detailsOpen}
              onClick={() => setDetailsOpen(!detailsOpen)}
            >
              {detailsOpen ? (
                <ChevronDown size={12} />
              ) : (
                <ChevronRight size={12} />
              )}
              Details
            </button>
            {detailsOpen && (
              <div className="td-details-body">
                {selected.request && selected.request !== selected.title && (
                  <p className="td-request">{selected.request}</p>
                )}
                <p className="td-facts">
                  <span
                    className={selected.tierFallback ? "td-warn" : undefined}
                  >
                    {formatTaskTier(selected)}
                  </span>
                  {selected.baseRef && (
                    <span className="td-branch" title={selected.baseRef}>
                      {" · "}from {selected.baseRef}
                    </span>
                  )}
                  {selected.variant?.land && (
                    <span className="td-branch" title={selected.landedSha}>
                      {" · "}
                      {selected.landedSha
                        ? `landed ${selected.landedSha.slice(0, 8)}`
                        : selected.status === "landing"
                          ? "landing: waiting for a clean checkout"
                          : "lands on its base"}
                    </span>
                  )}
                </p>
                {settings && (
                  <p className="td-facts">
                    Variant: {variantLabel(selected, settings.experiments)}
                  </p>
                )}
                <TaskCostLine
                  task={selected}
                  subtasks={subtasksCost}
                  legacy={legacy}
                />
                {children.length > 0 && (
                  <div className="td-subtasks">
                    <span className="td-eyebrow">SUBTASKS</span>
                    {children.map((child) => {
                      const tone = statusTone(child);
                      const after = dependencyTitles(child, tasks);
                      return (
                        <button
                          key={child.id}
                          className="td-subtask-row"
                          onClick={() => onOpen(child.id)}
                        >
                          <span className={`status-dot ${tone}`} />
                          <span className="td-subtask-title">
                            {child.title}
                          </span>
                          {after.length > 0 && (
                            <span className="td-faint">
                              after {after.join(", ")}
                            </span>
                          )}
                          {child.queueReason && (
                            <span className="td-faint">
                              {child.queueReason}
                            </span>
                          )}
                          <span className="td-faint">
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
                    act(() =>
                      orchestratorClient.taskOverturn(
                        selected.id,
                        index,
                        answer,
                      ),
                    )
                  }
                />
                {selected.decisions.length > 0 && (
                  <div className="td-decisions">
                    <span className="td-eyebrow">DECISIONS</span>
                    {selected.decisions.map((line, index) => {
                      const parsed = decisionTag(line);
                      return (
                        <p key={index} className="td-decision-line">
                          {parsed && (
                            <span className="td-decision-tag">
                              {parsed.tag}
                            </span>
                          )}
                          {parsed ? parsed.text : line}
                        </p>
                      );
                    })}
                  </div>
                )}
                {logLines && logLines.length > 0 && (
                  <pre className="td-log">{logLines.join("\n")}</pre>
                )}
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}
