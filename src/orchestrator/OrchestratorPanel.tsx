import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Plus,
  Square,
  Trash2,
  X,
} from "lucide-react";
import "./orchestrator.css";
import { orchestratorClient } from "./client";
import {
  applyOrchestratorEvent,
  attemptDurationMs,
  criteriaMet,
  emptyLiveState,
  formatCost,
  formatDuration,
  latestAttempt,
  statusLabel,
  totalDurationMs,
  upsertTask,
  type OrchestratorLiveState,
} from "./helpers";
import type { Attempt, PreflightResult, Settings, Task } from "./types";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type DaemonState = "loading" | "ready" | "not-built" | "unavailable";

function classifyError(message: string): DaemonState {
  return message.includes("is not built") ? "not-built" : "unavailable";
}

function statusDot(task: Task): string {
  switch (task.status) {
    case "running":
      return "green";
    case "waiting":
      return "yellow";
    case "failed":
    case "stopped":
      return "red";
    default:
      return "";
  }
}

/** Same idea as `statusDot`, one level down: a single attempt's own status. */
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
          {attempt.harness} · {attempt.model || attempt.routeId}
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
  onAnswer,
}: {
  task: Task;
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
        <button className="primary" type="submit" disabled={!freeText.trim()}>
          Answer
        </button>
      </form>
    </div>
  );
}

function NewTaskForm({
  repo,
  onCreated,
  onCancel,
}: {
  repo: string;
  onCreated(task: Task): void;
  onCancel(): void;
}) {
  const [title, setTitle] = useState("");
  const [goal, setGoal] = useState("");
  const [criteriaText, setCriteriaText] = useState("");
  const [verifyText, setVerifyText] = useState("");
  const [branch, setBranch] = useState("");
  const [preflight, setPreflight] = useState<PreflightResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  const criteria = useMemo(
    () =>
      criteriaText
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean),
    [criteriaText],
  );
  const verify = useMemo(
    () =>
      verifyText
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean),
    [verifyText],
  );

  async function runPreflight() {
    if (!goal.trim()) return;
    setChecking(true);
    try {
      const result = await orchestratorClient.taskPreflight({
        goal,
        criteria,
        verify,
      });
      setPreflight(result);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setChecking(false);
    }
  }

  async function submit(start: boolean) {
    if (!goal.trim()) {
      setError("Describe the goal first.");
      return;
    }
    setSaving(true);
    setError("");
    try {
      const task = await orchestratorClient.taskCreate({
        repo,
        title: title.trim() || goal.trim().slice(0, 80),
        goal: goal.trim(),
        criteria,
        verify,
        // Only when the owner actually typed one - the daemon slugs a
        // branch from the title itself otherwise.
        branch: branch.trim() || undefined,
        start,
      });
      onCreated(task);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="orch-new-task">
      <div className="dialog-eyebrow">NEW TASK</div>
      <label>
        Title
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          placeholder="Short summary"
        />
      </label>
      <label>
        Goal
        <textarea
          value={goal}
          onChange={(event) => setGoal(event.target.value)}
          onBlur={runPreflight}
          rows={3}
          placeholder="What should be true when this is done?"
          required
        />
      </label>
      <label>
        Acceptance criteria (one per line)
        <textarea
          value={criteriaText}
          onChange={(event) => setCriteriaText(event.target.value)}
          onBlur={runPreflight}
          rows={3}
        />
      </label>
      <label>
        Verification commands (one per line)
        <textarea
          value={verifyText}
          onChange={(event) => setVerifyText(event.target.value)}
          onBlur={runPreflight}
          rows={2}
          placeholder="npm test -- export"
        />
      </label>
      <label>
        Branch
        <input
          value={branch}
          onChange={(event) => setBranch(event.target.value)}
          placeholder="Optional, derived from title"
        />
      </label>
      {preflight && (
        <div className="orch-preflight-result">
          {!preflight.available && (
            <p className="text-muted">
              Classifier unavailable — checked with rules only.
            </p>
          )}
          {preflight.checks.map((check) => (
            <p
              key={check.id}
              className={check.ok ? "orch-check-ok" : "orch-check-bad"}
            >
              {check.ok ? <Check size={12} /> : <X size={12} />} {check.label}
              {typeof check.p === "number" && ` (${check.p.toFixed(2)})`}
            </p>
          ))}
        </div>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      <div className="form-row orch-form-actions">
        <button
          type="button"
          className="secondary"
          onClick={runPreflight}
          disabled={checking || !goal.trim()}
        >
          {checking ? "Checking…" : "Check readiness"}
        </button>
        <button
          className="primary"
          disabled={saving}
          onClick={() => submit(true)}
        >
          {saving ? "Starting…" : "Start"}
        </button>
        <button
          className="orch-ghost"
          disabled={saving}
          onClick={() => submit(false)}
        >
          Save without starting
        </button>
        <button type="button" className="orch-ghost" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

export function OrchestratorPanel({ cwd }: { cwd: string }) {
  const [daemonState, setDaemonState] = useState<DaemonState>("loading");
  const [error, setError] = useState("");
  const [live, setLive] = useState<OrchestratorLiveState>(emptyLiveState);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [selectedId, setSelectedId] = useState("");
  const [showNewTask, setShowNewTask] = useState(false);
  const [busy, setBusy] = useState(false);
  // Which pane a narrow panel shows - ignored by the CSS above ~720px, where
  // both columns are always visible side by side.
  const [narrowView, setNarrowView] = useState<"list" | "detail">("list");

  useEffect(() => {
    let cancelled = false;
    // Settings rides along with the task list only for its display value
    // (maxAttempts, the configured reviewer) - a failure here shouldn't block
    // the task list from showing.
    Promise.all([
      orchestratorClient.taskList(cwd),
      orchestratorClient.settingsGet().catch(() => null),
    ])
      .then(([tasks, loadedSettings]) => {
        if (cancelled) return;
        setLive((old) => ({ ...old, tasks }));
        setSettings(loadedSettings);
        setDaemonState("ready");
        setError("");
      })
      .catch((e) => {
        if (cancelled) return;
        const message = errorText(e);
        setError(message);
        setDaemonState(classifyError(message));
      });
    return () => {
      cancelled = true;
    };
  }, [cwd]);

  useEffect(
    () =>
      // The waiting-task attention notice is raised by the main process's
      // subscribe relay (electron/orchestrator.cjs), not here: this panel
      // only needs to exist for a task's live state to update.
      window.bridge?.onOrchestrator((event) => {
        if (event.event === "task" && event.task.repo !== cwd) return;
        setLive((old) => applyOrchestratorEvent(old, event));
      }),
    [cwd],
  );

  const tasks = live.tasks;
  const selected = tasks.find((t) => t.id === selectedId) || tasks[0];

  async function act(action: () => Promise<Task>) {
    setBusy(true);
    try {
      const task = await action();
      setLive((old) => ({ ...old, tasks: upsertTask(old.tasks, task) }));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  if (daemonState === "loading")
    return <div className="loading">Connecting to the orchestrator…</div>;
  if (daemonState === "not-built")
    return (
      <div className="empty-state">
        <h2>The orchestrator isn't built yet.</h2>
        <p>
          Run <code>npm run build:orchd</code>, then reopen this panel.
        </p>
      </div>
    );
  if (daemonState === "unavailable")
    return (
      <div className="empty-state">
        <h2>Can't reach the orchestrator daemon.</h2>
        <p>{error || "It may still be starting."}</p>
      </div>
    );

  return (
    <div className={`orchestrator-panel view-${narrowView}`}>
      <div className="orch-tasks">
        <div className="orch-tasks-head">
          <span className="dialog-eyebrow">TASKS</span>
          <button
            className="icon-button"
            aria-label="New task"
            onClick={() => {
              setShowNewTask(true);
              setNarrowView("detail");
            }}
          >
            <Plus size={14} />
          </button>
        </div>
        {tasks.length === 0 && !showNewTask && (
          <p className="text-muted">No tasks yet.</p>
        )}
        {tasks.map((task) => {
          const tone = statusDot(task) || "muted";
          return (
            <button
              key={task.id}
              className={`orch-task-row ${task.id === selected?.id ? "selected" : ""}`}
              onClick={() => {
                setSelectedId(task.id);
                setShowNewTask(false);
                setNarrowView("detail");
              }}
            >
              <span className={`status-dot ${statusDot(task)}`} />
              <span className="orch-task-lines">
                <span className="orch-task-title">{task.title}</span>
                <span className={`orch-task-status tone-${tone}`}>
                  {statusLabel(task, settings?.maxAttempts)}
                </span>
              </span>
            </button>
          );
        })}
      </div>
      <div className="orch-detail">
        <button
          className="orch-back-button"
          onClick={() => setNarrowView("list")}
        >
          <ChevronLeft size={14} /> Back
        </button>
        {showNewTask || !selected ? (
          <NewTaskForm
            repo={cwd}
            onCancel={() => {
              setShowNewTask(false);
              setNarrowView("list");
            }}
            onCreated={(task) => {
              setLive((old) => ({
                ...old,
                tasks: upsertTask(old.tasks, task),
              }));
              setSelectedId(task.id);
              setShowNewTask(false);
            }}
          />
        ) : (
          <>
            <div className="orch-detail-head">
              <div className="orch-detail-title">
                <h3>{selected.title}</h3>
                <p className="orch-detail-meta">
                  attempt {latestAttempt(selected)?.n ?? 0}/
                  {settings?.maxAttempts ?? latestAttempt(selected)?.n ?? 0} ·{" "}
                  {formatDuration(totalDurationMs(selected))} ·{" "}
                  {formatCost(selected.costUsd)} ·{" "}
                  <span className="orch-branch" title={selected.branch}>
                    {selected.branch}
                  </span>
                </p>
              </div>
              <div className="orch-detail-actions">
                {(selected.status === "queued" ||
                  selected.status === "stopped" ||
                  selected.status === "failed" ||
                  selected.status === "done") && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() =>
                      act(() => orchestratorClient.taskStart(selected.id))
                    }
                  >
                    Start
                  </button>
                )}
                {(selected.status === "running" ||
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
                  className="icon-button orch-delete-button"
                  title="Delete task"
                  disabled={busy}
                  onClick={async () => {
                    if (
                      !window.confirm(
                        `Delete "${selected.title}"?\n\nThe worktree and branch are left in place; only the task record is removed.`,
                      )
                    )
                      return;
                    setBusy(true);
                    try {
                      await orchestratorClient.taskDelete(selected.id);
                      setLive((old) => ({
                        ...old,
                        tasks: old.tasks.filter((t) => t.id !== selected.id),
                      }));
                      setSelectedId("");
                    } catch (e) {
                      setError(errorText(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  <Trash2 size={13} />
                </button>
              </div>
            </div>
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
            {selected.question && (
              <QuestionCard
                task={selected}
                onAnswer={(answer) =>
                  act(() => orchestratorClient.taskAnswer(selected.id, answer))
                }
              />
            )}
            <div className="orch-attempts">
              {selected.attempts.map((attempt) => (
                <AttemptRow key={attempt.n} attempt={attempt} />
              ))}
              {settings?.review &&
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
            {live.logLines[selected.id]?.length > 0 && (
              <pre className="orch-log">
                {live.logLines[selected.id].join("\n")}
              </pre>
            )}
          </>
        )}
      </div>
    </div>
  );
}
