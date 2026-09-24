import { useEffect, useState } from "react";
import {
  Archive,
  ArchiveRestore,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  MessageSquare,
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
  attemptProgress,
  criteriaMet,
  emptyLiveState,
  formatCost,
  formatDuration,
  latestImplementAttempt,
  statusBadgeLabel,
  statusDetail,
  totalDurationMs,
  upsertTask,
  type OrchestratorLiveState,
} from "./helpers";
import type { Attempt, ChatThread, Settings, Task } from "./types";
import { ChatTranscript } from "../ChatTranscript";
import { ChipPicker } from "../ChipPicker";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type DaemonState = "loading" | "ready" | "not-built" | "unavailable";

/** What the main pane beside the task list shows: the orchestrator chat is
 * where tasks come from, a task is opened to watch or answer it, the archive
 * lists what's been hidden from the main list. */
type View =
  { kind: "chat" } | { kind: "task"; id: string } | { kind: "archive" };

function classifyError(message: string): DaemonState {
  return message.includes("is not built") ? "not-built" : "unavailable";
}

/** One tone per status family, shared by a row's dot and its pill badge:
 * in flight reads blue, needs-you yellow, finished well green, went wrong
 * red, not started yet muted. */
function statusTone(task: Task): string {
  switch (task.status) {
    case "drafting":
    case "running":
      return "blue";
    case "waiting":
      return "yellow";
    case "done":
      return "green";
    case "failed":
    case "stopped":
      return "red";
    default:
      return "muted";
  }
}

// What needs the owner first, then what's in flight, then queued, then
// whatever already finished either way.
const STATUS_RANK: Record<Task["status"], number> = {
  waiting: 0,
  drafting: 1,
  running: 1,
  queued: 2,
  done: 3,
  failed: 3,
  stopped: 3,
};

function sortTasks(tasks: Task[]): Task[] {
  return [...tasks].sort((a, b) => {
    const byStatus = STATUS_RANK[a.status] - STATUS_RANK[b.status];
    return byStatus !== 0 ? byStatus : b.updatedAt - a.updatedAt;
  });
}

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

/** The orchestrator agent's conversation for this repo. The daemon runs each
 * turn and keeps the thread, so a reply keeps coming and stays readable when
 * the window closes or the app restarts. */
function OrchestratorChat({
  cwd,
  hidden,
  settings,
  onRouteChange,
}: {
  cwd: string;
  hidden: boolean;
  /** `null` while Settings is still loading - the route chip waits for it
   * rather than guessing at a harness icon. */
  settings: Settings | null;
  onRouteChange: (routeId: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [sendError, setSendError] = useState("");

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .chatGet(cwd)
      .then((loaded) => !cancelled && setThread(loaded))
      .catch((e) => !cancelled && setSendError(errorText(e)));
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event === "chat" && event.thread.repo === cwd)
        setThread(event.thread);
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [cwd]);

  function submit() {
    const text = draft.trim();
    if (!text || thread?.busy) return;
    setSendError("");
    orchestratorClient
      .chatSend(cwd, text)
      .then(() => setDraft(""))
      .catch((e) => setSendError(errorText(e)));
  }
  const error = sendError || thread?.error;
  // "" means the standard tier's own route - resolved here so the chip can
  // still show a harness icon and a real label instead of a blank default.
  const routeId = settings?.orchestrator || "";
  const resolvedRoute = settings
    ? settings.routes.find((r) => r.id === routeId) ||
      settings.routes.find((r) => r.id === settings.tiers.standard)
    : undefined;
  const harness = resolvedRoute?.harness || "claude";

  return (
    // Kept mounted behind the other views so a half-typed message and the
    // scroll position survive a look at a task.
    <div className="orch-chat" hidden={hidden}>
      <ChatTranscript
        messages={thread?.messages}
        busy={thread?.busy}
        note={thread?.note}
        error={error}
        welcome={
          <>
            <h2>Talk to the orchestrator</h2>
            <p>
              Describe what should happen - it creates, checks and answers tasks
              on your behalf.
            </p>
          </>
        }
      />
      <div className="chat-composer">
        <textarea
          rows={1}
          aria-label="Message the orchestrator"
          placeholder="Describe a task…"
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
            const el = event.target;
            el.style.height = "";
            el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
          }}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing
            ) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="chat-toolbar">
          {settings && (
            <ChipPicker
              icon={
                <img
                  className="harness-icon"
                  src={`./agents/${harness}.svg`}
                  width={12}
                  height={12}
                  alt=""
                />
              }
              label={
                routeId ? (resolvedRoute?.label ?? routeId) : "Standard route"
              }
              ariaLabel="Orchestrator route"
              value={routeId}
              onChange={onRouteChange}
              options={[
                { value: "", label: "Standard route" },
                ...settings.routes.map((r) => ({
                  value: r.id,
                  label: r.label,
                })),
              ]}
            />
          )}
          <span className="chat-toolbar-right">
            {thread?.busy ? (
              <button
                className="send enabled"
                aria-label="Stop response"
                onClick={() => void orchestratorClient.chatCancel(cwd)}
              >
                <Square size={12} />
              </button>
            ) : (
              <button
                className={`send ${draft.trim() ? "enabled" : ""}`}
                aria-label="Send message"
                disabled={!draft.trim()}
                onClick={submit}
              >
                <ArrowUp size={16} />
              </button>
            )}
          </span>
        </div>
      </div>
    </div>
  );
}

/** '+ New task' creates a task directly through `task.create`, not a
 * chat round-trip with the orchestrator agent. It prefers the `request`
 * form (the planner drafts title/goal/criteria/verify), but a disabled
 * planner shouldn't dead-end the control - falling back to the plain
 * title/goal form keeps it working either way. */
async function createTask(cwd: string, text: string): Promise<Task> {
  try {
    return await orchestratorClient.taskCreate(cwd, {
      request: text,
      start: true,
    });
  } catch (e) {
    if (!errorText(e).includes("planner is disabled")) throw e;
    return orchestratorClient.taskCreate(cwd, {
      title: text.length > 60 ? `${text.slice(0, 59)}…` : text,
      goal: text,
      start: true,
    });
  }
}

export function OrchestratorPanel({ cwd }: { cwd: string }) {
  const [daemonState, setDaemonState] = useState<DaemonState>("loading");
  const [error, setError] = useState("");
  const [live, setLive] = useState<OrchestratorLiveState>(emptyLiveState);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [view, setView] = useState<View | null>(null);
  const [busy, setBusy] = useState(false);
  // Which pane a narrow panel shows - ignored by the CSS above ~640px, where
  // the list and the main pane sit side by side.
  const [narrowMain, setNarrowMain] = useState(true);
  const [reload, setReload] = useState(0);
  const [creatingTask, setCreatingTask] = useState(false);
  const [taskDraft, setTaskDraft] = useState("");
  const [creatingBusy, setCreatingBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // Settings rides along with the task list only for its display value
    // (maxAttempts, the configured reviewer) - a failure here shouldn't block
    // the task list from showing.
    Promise.all([
      orchestratorClient.taskList(cwd, true),
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
  }, [cwd, reload]);

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

  const tasks = sortTasks(live.tasks.filter((t) => !t.archived));
  const archivedTasks = sortTasks(live.tasks.filter((t) => t.archived));
  const picked =
    view?.kind === "task" ? tasks.find((t) => t.id === view.id) : undefined;
  // Nothing picked (or the picked task was deleted): the orchestrator chat.
  const current: View =
    view && (view.kind !== "task" || picked) ? view : { kind: "chat" };
  const selected =
    current.kind === "task"
      ? (picked ?? tasks.find((t) => t.id === current.id))
      : undefined;
  const waitingCount = tasks.filter((t) => t.status === "waiting").length;

  function open(next: View) {
    setView(next);
    setNarrowMain(true);
  }

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

  function setOrchestratorRoute(routeId: string) {
    if (!settings) return;
    const next = { ...settings, orchestrator: routeId };
    setSettings(next);
    orchestratorClient
      .settingsSet(next)
      .then(setSettings)
      .catch((e) => setError(errorText(e)));
  }

  async function submitNewTask() {
    const text = taskDraft.trim();
    if (!text || creatingBusy) return;
    setCreatingBusy(true);
    setError("");
    try {
      const task = await createTask(cwd, text);
      setLive((old) => ({ ...old, tasks: upsertTask(old.tasks, task) }));
      setTaskDraft("");
      setCreatingTask(false);
      open({ kind: "task", id: task.id });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setCreatingBusy(false);
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
        <button
          className="secondary"
          onClick={() => {
            setDaemonState("loading");
            setReload((n) => n + 1);
          }}
        >
          Retry
        </button>
      </div>
    );

  return (
    <div
      className={`orchestrator-panel ${narrowMain ? "view-main" : "view-list"}`}
    >
      <div className="orch-tasks">
        <button
          className={`orch-nav-row ${current.kind === "chat" ? "selected" : ""}`}
          onClick={() => open({ kind: "chat" })}
        >
          <MessageSquare size={14} /> Orchestrator
        </button>
        <button
          className="orch-nav-row"
          aria-expanded={creatingTask}
          onClick={() => setCreatingTask((v) => !v)}
        >
          <Plus size={14} /> New task
        </button>
        {creatingTask && (
          <form
            className="orch-new-task"
            onSubmit={(event) => {
              event.preventDefault();
              void submitNewTask();
            }}
          >
            <input
              autoFocus
              value={taskDraft}
              onChange={(event) => setTaskDraft(event.target.value)}
              placeholder="What should happen?"
              aria-label="New task request"
              disabled={creatingBusy}
            />
            <button
              className="icon-button"
              type="submit"
              aria-label="Create task"
              disabled={creatingBusy || !taskDraft.trim()}
            >
              <ArrowUp size={14} />
            </button>
            <button
              className="icon-button"
              type="button"
              aria-label="Cancel new task"
              onClick={() => {
                setCreatingTask(false);
                setTaskDraft("");
              }}
            >
              <X size={13} />
            </button>
          </form>
        )}
        <button
          className={`orch-nav-row ${current.kind === "archive" ? "selected" : ""}`}
          onClick={() => open({ kind: "archive" })}
        >
          <Archive size={14} /> Archive
          {archivedTasks.length > 0 && (
            <span className="orch-archive-count">{archivedTasks.length}</span>
          )}
        </button>
        <div className="orch-tasks-head">
          <span className="dialog-eyebrow">TASKS</span>
          {waitingCount > 0 && (
            <span className="orch-waiting-count">{waitingCount} need you</span>
          )}
        </div>
        {tasks.length === 0 && <p className="orch-empty-list">No tasks yet.</p>}
        {tasks.map((task) => {
          const tone = statusTone(task);
          const progress = attemptProgress(task, settings?.maxAttempts);
          const detail = statusDetail(task);
          return (
            <button
              key={task.id}
              className={`orch-task-row ${task.id === selected?.id ? "selected" : ""}`}
              onClick={() => open({ kind: "task", id: task.id })}
            >
              <span className={`status-dot ${tone}`} />
              <span className="orch-task-lines">
                <span className="orch-task-title">{task.title}</span>
                <span className="orch-task-badges">
                  <span className={`orch-status-badge tone-${tone}`}>
                    {statusBadgeLabel(task)}
                  </span>
                  {progress && (
                    <span className="orch-attempt-progress">
                      attempt {progress}
                    </span>
                  )}
                </span>
                {detail && (
                  <span className={`orch-task-status tone-${tone}`}>
                    {detail}
                  </span>
                )}
              </span>
            </button>
          );
        })}
      </div>
      <div className="orch-main">
        <button
          className="orch-back-button"
          onClick={() => setNarrowMain(false)}
        >
          <ChevronLeft size={14} /> Tasks
          {waitingCount > 0 && (
            <span className="orch-waiting-count">{waitingCount} need you</span>
          )}
        </button>
        {error && (
          <div className="orch-error" role="alert">
            <span>{error}</span>
            <button
              className="icon-button"
              aria-label="Dismiss error"
              onClick={() => setError("")}
            >
              <X size={13} />
            </button>
          </div>
        )}
        <OrchestratorChat
          cwd={cwd}
          hidden={current.kind !== "chat"}
          settings={settings}
          onRouteChange={setOrchestratorRoute}
        />
        {current.kind === "archive" && (
          <div className="orch-archive">
            <h3>Archive</h3>
            {archivedTasks.length === 0 ? (
              <p className="orch-empty-list">No archived tasks.</p>
            ) : (
              archivedTasks.map((task) => (
                <div key={task.id} className="orch-archive-row">
                  <span className="orch-task-title">{task.title}</span>
                  <button
                    className="icon-button"
                    title="Restore task"
                    disabled={busy}
                    onClick={() =>
                      act(() => orchestratorClient.taskUnarchive(task.id))
                    }
                  >
                    <ArchiveRestore size={13} />
                  </button>
                </div>
              ))
            )}
          </div>
        )}
        {selected && (
          <div className="orch-detail">
            <div className="orch-detail-head">
              <div className="orch-detail-title">
                <h3>{selected.title}</h3>
                <p className="orch-detail-meta">
                  attempt {latestImplementAttempt(selected)?.n ?? 0}/
                  {settings?.maxAttempts ??
                    latestImplementAttempt(selected)?.n ??
                    0}{" "}
                  · {formatDuration(totalDurationMs(selected))} ·{" "}
                  <span title="API list price the CLI reports, cache included; a subscription is not billed per token">
                    {formatCost(selected.costUsd)}
                  </span>{" "}
                  ·{" "}
                  <span className="orch-branch" title={selected.branch}>
                    {selected.branch}
                  </span>
                </p>
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
                      selected.status === "stopped" ||
                      selected.status === "failed"
                        ? "primary"
                        : "secondary"
                    }
                    disabled={busy}
                    onClick={() =>
                      act(() => orchestratorClient.taskStart(selected.id))
                    }
                  >
                    {selected.status === "drafting" ||
                    selected.status === "queued"
                      ? "Start"
                      : "Run again"}
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
                      setView(null);
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
            {/* The one thing blocking the task goes first, above everything
             * that only describes it. */}
            {selected.question && (
              <QuestionCard
                key={selected.id}
                task={selected}
                disabled={busy}
                onAnswer={(answer) =>
                  act(() => orchestratorClient.taskAnswer(selected.id, answer))
                }
              />
            )}
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
          </div>
        )}
      </div>
    </div>
  );
}
