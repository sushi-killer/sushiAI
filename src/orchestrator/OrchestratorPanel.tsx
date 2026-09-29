import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import {
  Archive,
  ArchiveRestore,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CornerDownRight,
  MessageSquare,
  MessagesSquare,
  Plus,
  Square,
  Trash2,
  X,
} from "lucide-react";
import "./orchestrator.css";
import type { OrchestratorView } from "../types";
import type { TaskTarget } from "./notices";
import { pendingReveal, subscribeReveal } from "./reveal";
import { orchestratorClient } from "./client";
import {
  applyOrchestratorEvent,
  attemptDurationMs,
  attemptProgress,
  childrenOf,
  costByStage,
  criteriaMet,
  dependencyTitles,
  emptyLiveState,
  fingerprintLabel,
  formatCost,
  formatDuration,
  formatTaskTier,
  implementAttemptCount,
  messageThreads,
  participantLabel,
  statusBadgeLabel,
  statusDetail,
  taskCreateParams,
  totalDurationMs,
  upsertMessage,
  upsertTask,
  variantLabel,
  type MessageThread,
  type OrchestratorLiveState,
} from "./helpers";
import type {
  Attempt,
  ChatSessionList,
  ChatThread,
  Message,
  Settings,
  Task,
} from "./types";
import { ChatTranscript } from "../ChatTranscript";
import { ChipPicker } from "../ChipPicker";
import { RichText } from "../agents/AgentsView";
import {
  EvolutionProposals,
  RecurringFailures,
  RepoNotes,
  SpendBlock,
  TaskCostLine,
  TaskTimelineBar,
} from "./TaskInsights";

/** Electron wraps a rejected IPC call as "Error invoking remote method
 * 'orchestrator': Error: <daemon message>"; the owner only needs the last
 * part. */
function errorText(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(
    /^Error invoking remote method '[^']*': (Error: )?/,
    "",
  );
}

type DaemonState = "loading" | "ready" | "not-built" | "unavailable";

/** What the main pane beside the task list shows: the orchestrator chat is
 * where tasks come from, a task is opened to watch or answer it, the archive
 * lists what's been hidden from the main list, and "messages" watches the
 * agent-to-agent threads the daemon keeps. */
type View = OrchestratorView;

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
    case "landing":
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
  landing: 1,
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

/** The orchestrator agent's conversations for this repo. The daemon runs each
 * turn and keeps every session, so a reply keeps coming and stays readable
 * when the window closes or the app restarts. A live reply pins the current
 * session: New chat, switching and Clear are disabled meanwhile, and the
 * daemon's refusal still shows if a click races the reply's first event. */
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
  const [list, setList] = useState<ChatSessionList | null>(null);
  const [sendError, setSendError] = useState("");

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      orchestratorClient.chatGet(cwd),
      orchestratorClient.chatList(cwd),
    ])
      .then(([loaded, sessions]) => {
        if (cancelled) return;
        setThread(loaded);
        setList(sessions);
      })
      .catch((e) => !cancelled && setSendError(errorText(e)));
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event === "chat" && event.thread.repo === cwd) {
        // A refusal ("still answering") is stale once the reply is in.
        if (!event.thread.busy) setSendError("");
        setThread(event.thread);
        setList({ current: event.current, sessions: event.sessions });
      }
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
  /** New chat, switch and Clear all answer with the now-current session;
   * the chat event that follows refreshes the session list. */
  function changeSession(request: Promise<ChatThread>) {
    setSendError("");
    request.then(setThread).catch((e) => setSendError(errorText(e)));
  }
  const error = sendError || thread?.error;
  const busy = Boolean(thread?.busy);
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
      <div className="orch-sessions" aria-label="Chat sessions">
        <button
          className="orch-nav-row"
          disabled={busy}
          title={busy ? "Wait for the reply to finish" : undefined}
          onClick={() => changeSession(orchestratorClient.chatNew(cwd))}
        >
          <Plus size={14} /> New chat
        </button>
        {/* Newest first, so a new chat lands right under its button. */}
        {[...(list?.sessions ?? [])].reverse().map((session) => {
          const current = session.id === list?.current;
          return (
            <div
              key={session.id}
              className={`orch-session-row ${current ? "selected" : ""}`}
            >
              <button
                className="orch-session-open"
                aria-current={current || undefined}
                disabled={busy && !current}
                title={
                  busy && !current ? "Wait for the reply to finish" : undefined
                }
                onClick={() =>
                  !current &&
                  changeSession(orchestratorClient.chatSwitch(cwd, session.id))
                }
              >
                <MessageSquare size={13} />
                <span
                  className={`orch-session-title ${session.title ? "" : "untitled"}`}
                >
                  {session.title || "New chat"}
                </span>
                {session.busy && (
                  <span
                    className="status-dot blue pulse"
                    aria-label="Answering"
                  />
                )}
              </button>
              {current && (
                <button
                  className="icon-button orch-session-clear"
                  aria-label="Clear chat"
                  disabled={busy}
                  title={
                    busy ? "Wait for the reply to finish" : "Clear this chat"
                  }
                  onClick={() =>
                    changeSession(orchestratorClient.chatClear(cwd))
                  }
                >
                  <Trash2 size={13} />
                </button>
              )}
            </div>
          );
        })}
      </div>
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
async function createTask(
  cwd: string,
  text: string,
  base: string,
): Promise<Task> {
  try {
    return await orchestratorClient.taskCreate(
      cwd,
      taskCreateParams({ request: text, start: true }, base),
    );
  } catch (e) {
    if (!errorText(e).includes("planner is disabled")) throw e;
    return orchestratorClient.taskCreate(
      cwd,
      taskCreateParams(
        {
          title: text.length > 60 ? `${text.slice(0, 59)}…` : text,
          goal: text,
          start: true,
        },
        base,
      ),
    );
  }
}

/** A small badge on a question to the orchestrator - the same look as a
 * decision tag (`orch-decision-tag`). A reply already carries its own
 * "↳ reply" marker, so it gets no second badge. */
function MessageKindTag({ kind }: { kind: Message["kind"] }) {
  if (kind !== "question") return null;
  return (
    <span className="orch-decision-tag orch-message-kind-question">{kind}</span>
  );
}

/** One message inside a thread card. A reply reads as answering its question
 * with an indent and a small "↳ reply" marker, rather than just another line
 * in the same column as everything else. */
function MessageRow({ message, tasks }: { message: Message; tasks: Task[] }) {
  const when = new Date(message.ts);
  return (
    <div
      className={`orch-message-row ${message.kind === "reply" ? "orch-message-reply" : ""}`}
    >
      {message.kind === "reply" && (
        <span className="orch-message-reply-marker">
          <CornerDownRight size={12} /> reply
        </span>
      )}
      <div className="orch-message-meta">
        <span className="orch-message-from">
          {participantLabel(message.from, tasks)}
        </span>
        <MessageKindTag kind={message.kind} />
        <time
          className="orch-message-time"
          dateTime={when.toISOString()}
          title={when.toLocaleString("en-US")}
        >
          {when.toLocaleTimeString("en-US", {
            hour: "2-digit",
            minute: "2-digit",
          })}
        </time>
      </div>
      <div className="orch-message-text">
        <RichText text={message.text} />
      </div>
      <span
        className={`orch-message-delivery ${message.delivered ? "delivered" : "pending"}`}
      >
        {message.delivered ? "delivered" : "pending — arrives on its next turn"}
      </span>
    </div>
  );
}

/** One conversation between two participants (a task and the orchestrator,
 * or two tasks), newest thread first, its own messages oldest first. */
function MessageThreadCard({
  thread,
  tasks,
}: {
  thread: MessageThread;
  tasks: Task[];
}) {
  const [a, b] = thread.participants;
  return (
    <div className="orch-thread-card">
      <div className="orch-thread-head">
        <span className="orch-thread-title">
          {participantLabel(a, tasks)} ↔ {participantLabel(b, tasks)}
        </span>
        <span className="orch-thread-count">
          {thread.messages.length} message
          {thread.messages.length === 1 ? "" : "s"}
        </span>
      </div>
      {thread.messages.map((message) => (
        <MessageRow key={message.id} message={message} tasks={tasks} />
      ))}
    </div>
  );
}

/** Agent-to-agent (and agent-to-orchestrator) message threads the daemon
 * keeps for this repo - read-only, the owner only watches. Self-contained
 * like `OrchestratorChat`: it loads and live-updates its own state instead of
 * riding the panel's shared `live` state (`applyOrchestratorEvent` leaves a
 * `message` event untouched, same as it does for `chat`). */
function MessagesView({
  cwd,
  tasks,
  hidden,
}: {
  cwd: string;
  tasks: Task[];
  hidden: boolean;
}) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [loadError, setLoadError] = useState("");

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .messageList(cwd)
      .then((loaded) => !cancelled && setMessages(loaded))
      .catch((e) => !cancelled && setLoadError(errorText(e)));
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event === "message" && event.message.repo === cwd)
        setMessages((old) => upsertMessage(old, event.message));
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [cwd]);

  const threads = messageThreads(messages);

  return (
    <div className="orch-messages" hidden={hidden}>
      {loadError && (
        <div className="orch-error" role="alert">
          <span>{loadError}</span>
        </div>
      )}
      {threads.length === 0 ? (
        <div className="chat-welcome">
          <h2>Agent messages</h2>
          <p>
            Tasks message each other and ask the orchestrator here - watch each
            thread as it happens, no need to answer on their behalf.
          </p>
        </div>
      ) : (
        <div className="orch-thread-list">
          {threads.map((thread) => (
            <MessageThreadCard key={thread.key} thread={thread} tasks={tasks} />
          ))}
        </div>
      )}
    </div>
  );
}

export function OrchestratorPanel({
  cwd,
  view: savedView,
  onViewChange,
}: {
  cwd: string;
  /** The view the panel had when it was last open; seeds its own state. */
  view?: OrchestratorView;
  /** Reports the view after each change so it can be saved on the panel. */
  onViewChange(view: OrchestratorView | undefined): void;
}) {
  const [daemonState, setDaemonState] = useState<DaemonState>("loading");
  const [error, setError] = useState("");
  const [live, setLive] = useState<OrchestratorLiveState>(emptyLiveState);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [view, setViewState] = useState<View | null>(savedView ?? null);
  const report = useRef(onViewChange);
  report.current = onViewChange;
  const setView = useCallback((next: View | null) => {
    setViewState(next);
    report.current(next ?? undefined);
  }, []);
  const [busy, setBusy] = useState(false);
  // Which pane a narrow panel shows - ignored by the CSS above ~640px, where
  // the list and the main pane sit side by side.
  const [narrowMain, setNarrowMain] = useState(true);
  const [reload, setReload] = useState(0);
  const [creatingTask, setCreatingTask] = useState(false);
  const [taskDraft, setTaskDraft] = useState("");
  const [baseBranchDraft, setBaseBranchDraft] = useState("");
  const [creatingBusy, setCreatingBusy] = useState(false);
  // Tracked here, separately from `MessagesView`'s own copy, only so the nav
  // row can show a pending count before that view is ever opened - the same
  // reason `waitingCount` below reads from `live.tasks` instead of the task
  // detail pane.
  const [messages, setMessages] = useState<Message[]>([]);

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
      // Task notices are raised by the main process's subscribe relay
      // (electron/orchestrator.cjs), not here: this panel only needs to
      // exist for a task's live state to update.
      window.bridge?.onOrchestrator((event) => {
        if (event.event === "task" && event.task.repo !== cwd) return;
        if (event.event === "message") {
          if (event.message.repo === cwd)
            setMessages((old) => upsertMessage(old, event.message));
          return;
        }
        setLive((old) => applyOrchestratorEvent(old, event));
      }),
    [cwd],
  );

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .messageList(cwd)
      .then((loaded) => !cancelled && setMessages(loaded))
      .catch(() => {
        // Silent: the badge just stays at 0 until the next successful load,
        // never blocking the rest of the panel from showing.
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, reload]);

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
  const children = selected ? childrenOf(live.tasks, selected.id) : [];
  const parentTask = selected?.parent
    ? live.tasks.find((t) => t.id === selected.parent)
    : undefined;
  const waitsFor = selected ? dependencyTitles(selected, live.tasks) : [];
  const waitingCount = tasks.filter((t) => t.status === "waiting").length;
  const pendingMessages = messages.filter((m) => !m.delivered).length;

  function open(next: View) {
    setView(next);
    setNarrowMain(true);
  }

  // A notice's "open this task" lands here: select it, then bring its
  // question card (needs input), its report (done) or its attempts (failed) into view.
  const rootRef = useRef<HTMLDivElement>(null);
  const [scrollTo, setScrollTo] = useState<TaskTarget | null>(null);
  const applied = useRef<TaskTarget | null>(null);
  useEffect(() => {
    const take = () => {
      const target = pendingReveal(cwd);
      if (!target || applied.current === target) return;
      applied.current = target;
      setView({ kind: "task", id: target.taskId });
      setNarrowMain(true);
      setScrollTo(target);
    };
    take();
    return subscribeReveal(take);
  }, [cwd, setView]);
  useEffect(() => {
    if (!scrollTo || selected?.id !== scrollTo.taskId) return;
    const root = rootRef.current;
    const element =
      (scrollTo.focus === "question" &&
        root?.querySelector(".orch-question")) ||
      (scrollTo.focus === "report" && root?.querySelector(".orch-report")) ||
      root?.querySelector(".orch-attempts");
    if (!element) return;
    element.scrollIntoView({ block: "center" });
    setScrollTo(null);
  }, [scrollTo, selected?.id, live.tasks]);

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
      const task = await createTask(cwd, text, baseBranchDraft);
      setLive((old) => ({ ...old, tasks: upsertTask(old.tasks, task) }));
      setTaskDraft("");
      setBaseBranchDraft("");
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
      ref={rootRef}
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
            <label>
              Request
              <input
                autoFocus
                value={taskDraft}
                onChange={(event) => setTaskDraft(event.target.value)}
                placeholder="What should happen?"
                disabled={creatingBusy}
              />
            </label>
            <div className="orch-new-task-row">
              <label>
                Base branch (optional)
                <input
                  value={baseBranchDraft}
                  onChange={(event) => setBaseBranchDraft(event.target.value)}
                  placeholder="Defaults to the current branch"
                  disabled={creatingBusy}
                />
              </label>
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
                  setBaseBranchDraft("");
                }}
              >
                <X size={13} />
              </button>
            </div>
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
        <button
          className={`orch-nav-row ${current.kind === "messages" ? "selected" : ""}`}
          onClick={() => open({ kind: "messages" })}
        >
          <MessagesSquare size={14} /> Messages
          {pendingMessages > 0 && (
            <span className="orch-waiting-count">
              {pendingMessages} pending
            </span>
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
        {current.kind === "chat" && (
          <SpendBlock
            cwd={cwd}
            refresh={live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join()}
          />
        )}
        {current.kind === "chat" && (
          <RecurringFailures
            cwd={cwd}
            refresh={live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join()}
            onOpenTask={(id) => open({ kind: "task", id })}
          />
        )}
        {current.kind === "chat" && (
          <>
            <EvolutionProposals
              cwd={cwd}
              refresh={live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join()}
            />
            <RepoNotes
              cwd={cwd}
              refresh={live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join()}
            />
          </>
        )}
        <OrchestratorChat
          cwd={cwd}
          hidden={current.kind !== "chat"}
          settings={settings}
          onRouteChange={setOrchestratorRoute}
        />
        <MessagesView
          cwd={cwd}
          tasks={tasks}
          hidden={current.kind !== "messages"}
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
                {(parentTask || waitsFor.length > 0) && (
                  <p className="orch-detail-meta">
                    {parentTask && (
                      <button
                        className="orch-parent-link"
                        onClick={() =>
                          open({ kind: "task", id: parentTask.id })
                        }
                      >
                        Part of {parentTask.title}
                      </button>
                    )}
                    {parentTask && waitsFor.length > 0 && " · "}
                    {waitsFor.length > 0 && `after ${waitsFor.join(", ")}`}
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
                tasks={live.tasks}
                disabled={busy}
                onOpen={(id) => open({ kind: "task", id })}
                onLeadTouch={(touched, note) =>
                  act(() =>
                    orchestratorClient.taskLeadTouch(
                      selected.id,
                      touched,
                      note,
                    ),
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
                  const after = dependencyTitles(child, live.tasks);
                  return (
                    <button
                      key={child.id}
                      className="orch-subtask-row"
                      onClick={() => open({ kind: "task", id: child.id })}
                    >
                      <span className={`status-dot ${tone}`} />
                      <span className="orch-task-title">{child.title}</span>
                      {after.length > 0 && (
                        <span className="orch-subtask-after">
                          after {after.join(", ")}
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
                act(() =>
                  orchestratorClient.taskOverturn(selected.id, index, answer),
                )
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
