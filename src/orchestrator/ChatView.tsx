import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  ArrowRight,
  CornerDownRight,
  Lightbulb,
  ListChecks,
  MessageSquare,
  MoreHorizontal,
  Plus,
  Search,
  Sparkles,
  SquareTerminal,
} from "lucide-react";
import { useOrchestratorClient, useOrchestratorHost } from "./hostContext";
import { hostOf } from "./hosts";
import {
  groupSessions,
  matchesQuery,
  isUnread,
  canSendEdit,
  composeAnswers,
  composerPlaceholder,
  editKeyAction,
  isAfterEdit,
  pickedAnswers,
  lastMessageText,
  modeLabel,
  sessionTime,
  taskRefs,
} from "./chatModel";
import {
  errorText,
  formatCost,
  participantLabel,
  taskReason,
  taskTone,
  upsertMessage,
  type Tone,
} from "./helpers";
import type {
  ChatMessage,
  ChatMode,
  ChatSessionList,
  ChatSessionSummary,
  ChatThread,
  Message,
  Settings,
  Task,
} from "./types";
import { RichText } from "../agents/AgentsView";
import { Composer, OrchestratorRouteChip } from "./Composer";
import { ProposalCard } from "./ProposalCard";
import { ToolConfirm } from "./ToolConfirm";
import { Chip, Tag } from "./ui";
import "./chat.css";

const STARTERS = [
  "What is running and what needs me?",
  "Why did the last task fail?",
  "Land everything that passed review",
];

/** Under the starters of an empty chat: what the other two modes are for. */
const MODE_HINTS = [
  ["Brainstorm", "turns an idea into tasks, one question at a time"],
  ["Plan", "splits a goal into ordered tasks you confirm"],
] as const;

/** "Orchestrator · claude-sonnet": the route the chat runs on, by model. */
function routeLabel(settings: Settings): string {
  const id = settings.orchestrator || settings.tiers.standard;
  const route = settings.routes.find((r) => r.id === id);
  const name = route?.label || id;
  return name ? `Orchestrator · ${name}` : "Orchestrator";
}

/** A known task the orchestrator named, as a card that opens it. */
function TaskRefCard({
  task,
  tasks,
  maxAttempts,
  onOpenTask,
}: {
  task: Task;
  tasks: Task[];
  maxAttempts?: number;
  onOpenTask?: (id: string) => void;
}) {
  const cost = task.costUsd ? formatCost(task.costUsd) : "";
  const sub = [taskReason(task, tasks, maxAttempts), cost]
    .filter(Boolean)
    .join(" · ");
  const body = (
    <>
      <span className={`ui-dot ui-tone-${taskTone(task)}`} />
      <span className="ochat-ref-body">
        <span className="ochat-ref-title" title={task.title}>
          {task.title}
        </span>
        <span className="ochat-ref-sub">{sub}</span>
      </span>
    </>
  );
  // Without a way to open the task the card still names it, unclickable.
  if (!onOpenTask) return <div className="ochat-ref">{body}</div>;
  return (
    <button
      type="button"
      className="ochat-ref"
      onClick={() => onOpenTask(task.id)}
    >
      {body}
      <ArrowRight size={12} className="ochat-ref-arrow" />
    </button>
  );
}

function Avatar() {
  return (
    <span className="ochat-avatar" aria-hidden>
      <Sparkles size={12} />
    </span>
  );
}

function OrchestratorTurn({
  cwd,
  message,
  nextUser,
  tasks,
  maxAttempts,
  busy,
  onOpenTask,
  onPick,
}: {
  cwd: string;
  message: ChatMessage;
  /** The owner's next message, to mark the option they picked. */
  nextUser: string | undefined;
  tasks: Task[];
  maxAttempts?: number;
  busy: boolean;
  onOpenTask?: (id: string) => void;
  onPick: (option: string, mode: ChatMode) => void;
}) {
  const refs = taskRefs(message.text, tasks);
  const label = modeLabel(message.mode);
  // orchd falls back to the question as the reply's text when the reply was
  // only a block; that question is not shown twice.
  const questions = (message.questions ?? []).filter(
    (q) => q.text !== message.text || q.options.length > 0,
  );
  // Two or more questions are answered together: chips only choose, the
  // button sends. A single question sends on its chip.
  const together = questions.length > 1;
  const [picks, setPicks] = useState<(string | undefined)[]>([]);
  const sentPicks = pickedAnswers(questions, nextUser);
  const shown = nextUser !== undefined ? sentPicks : picks;
  const answers = composeAnswers(questions, picks);
  const choose = (index: number, option: string) =>
    setPicks((current) =>
      questions.map((_, i) =>
        i !== index ? current[i] : current[i] === option ? undefined : option,
      ),
    );
  return (
    <div className="ochat-orch">
      <Avatar />
      <div className="ochat-orch-col">
        {label && (
          <span className="ochat-mode-label">
            {message.mode === "plan" ? (
              <ListChecks size={12} />
            ) : (
              <Lightbulb size={12} />
            )}
            {label}
          </span>
        )}
        <div className="ochat-orch-text">
          <RichText text={message.text} />
        </div>
        {questions.map((question, index) => (
          <div key={question.text} className="ochat-question">
            {question.text !== message.text && (
              <p className="ochat-question-text">{question.text}</p>
            )}
            {question.options.length > 0 && (
              <div className="ochat-question-opts">
                {question.options.map((option) => (
                  <Chip
                    key={option}
                    selected={
                      together ? shown[index] === option : nextUser === option
                    }
                    disabled={busy || (together && nextUser !== undefined)}
                    onClick={() =>
                      together
                        ? choose(index, option)
                        : onPick(option, message.mode ?? "chat")
                    }
                  >
                    {option}
                  </Chip>
                ))}
              </div>
            )}
          </div>
        ))}
        {together && nextUser === undefined && (
          <div>
            <button
              type="button"
              className="ui-button primary"
              disabled={busy || answers === null}
              onClick={() => answers && onPick(answers, message.mode ?? "chat")}
            >
              Send answers
            </button>
          </div>
        )}
        {message.action && (
          <ToolConfirm
            cwd={cwd}
            messageId={message.id}
            action={message.action}
            disabled={busy}
          />
        )}
        {message.proposal && (
          <ProposalCard
            cwd={cwd}
            message={message}
            proposal={message.proposal}
            tasks={tasks}
            disabled={busy}
            onOpenTask={onOpenTask}
          />
        )}
        {refs.map((task) => (
          <TaskRefCard
            key={task.id}
            task={task}
            tasks={tasks}
            maxAttempts={maxAttempts}
            onOpenTask={onOpenTask}
          />
        ))}
      </div>
    </div>
  );
}

/** The conversation column's head: title, route and the session menu. */
function ChatHead({
  title,
  settings,
  busy,
  canClear,
  onClear,
}: {
  title: string;
  settings: Settings | null;
  busy: boolean;
  canClear: boolean;
  onClear: () => void;
}) {
  const [menu, setMenu] = useState(false);
  return (
    <div className="ochat-head">
      <h2 className="ochat-head-title">{title}</h2>
      {settings && (
        <span className="ochat-route" title="Change it in the composer">
          <Sparkles size={12} />
          {routeLabel(settings)}
        </span>
      )}
      <div className="ochat-menu-wrap">
        <button
          type="button"
          className="icon-button ochat-menu-btn"
          aria-label="Chat actions"
          aria-haspopup="menu"
          aria-expanded={menu}
          onClick={() => setMenu((v) => !v)}
        >
          <MoreHorizontal size={16} />
        </button>
        {menu && (
          <div className="ochat-menu" role="menu">
            <button
              type="button"
              role="menuitem"
              disabled={busy || !canClear}
              title={busy ? "Wait for the reply to finish" : undefined}
              onClick={() => {
                setMenu(false);
                onClear();
              }}
            >
              Clear chat
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** The orchestrator conversation: the current chat session's turns and the
 * composer under them. */
function Conversation({
  cwd,
  thread,
  error,
  settings,
  tasks,
  mode,
  onModeChange,
  onRouteChange,
  onOpenTask,
  onSend,
  onEdit,
  onClear,
}: {
  cwd: string;
  thread: ChatThread | null;
  error: string;
  settings: Settings | null;
  tasks: Task[];
  mode: ChatMode;
  onModeChange: (mode: ChatMode) => void;
  onRouteChange: (routeId: string) => void;
  onOpenTask?: (id: string) => void;
  onSend: (text: string, mode: ChatMode) => Promise<boolean>;
  onEdit: (messageId: string, text: string, mode: ChatMode) => Promise<boolean>;
  onClear: () => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(
    null,
  );
  const endRef = useRef<HTMLDivElement>(null);
  const messages = thread?.messages ?? [];
  const busy = Boolean(thread?.busy);
  const last = messages.length ? messages[messages.length - 1].id : "";
  // A different session or a running turn closes the editor.
  const threadId = thread?.id;
  useEffect(() => setEditing(null), [threadId]);
  useEffect(() => {
    if (busy) setEditing(null);
  }, [busy]);
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [last, busy]);

  function submit() {
    const text = draft.trim();
    if (!text || busy) return;
    void onSend(text, mode).then((sent) => sent && setDraft(""));
  }
  function sendEdit(message: ChatMessage) {
    if (!editing || !canSendEdit(editing.text, busy)) return;
    void onEdit(message.id, editing.text.trim(), message.mode ?? "chat").then(
      (sent) => sent && setEditing(null),
    );
  }
  const shownError = error || thread?.error;
  const empty = messages.length === 0 && !busy;
  return (
    <>
      <ChatHead
        title={thread?.title || "New chat"}
        settings={settings}
        busy={busy}
        canClear={messages.length > 0}
        onClear={onClear}
      />
      <div className={`ochat-scroll ${empty ? "empty" : ""}`}>
        {empty ? (
          <div className="ochat-empty">
            <h3>Ask about your tasks</h3>
            <p>
              The orchestrator reads every task, its attempts and costs. It can
              start, stop, answer and land them for you.
            </p>
            <div className="ochat-starters">
              {STARTERS.map((text) => (
                <Chip
                  key={text}
                  disabled={!thread}
                  onClick={() => void onSend(text, mode)}
                >
                  {text}
                </Chip>
              ))}
            </div>
            <ul className="ochat-mode-hints">
              {MODE_HINTS.map(([name, hint]) => (
                <li key={name}>
                  <strong>{name}</strong> {hint}
                </li>
              ))}
            </ul>
          </div>
        ) : (
          messages.map((message, i) => {
            const dimmed = isAfterEdit(messages, editing?.id ?? null, i);
            if (message.role === "user")
              return (
                <div
                  key={message.id}
                  className={`ochat-you ${dimmed ? "dimmed" : ""}`}
                >
                  {editing?.id === message.id ? (
                    <MessageEditor
                      text={editing.text}
                      onChange={(text) => setEditing({ id: message.id, text })}
                      onCancel={() => setEditing(null)}
                      onSend={() => sendEdit(message)}
                    />
                  ) : (
                    <>
                      <div className="ochat-bubble">{message.text}</div>
                      <div className="ochat-you-actions">
                        {!busy && !editing && (
                          <button
                            type="button"
                            onClick={() =>
                              setEditing({ id: message.id, text: message.text })
                            }
                          >
                            Edit
                          </button>
                        )}
                        <button
                          type="button"
                          onClick={() =>
                            void navigator.clipboard?.writeText(message.text)
                          }
                        >
                          Copy
                        </button>
                      </div>
                    </>
                  )}
                </div>
              );
            return (
              <div key={message.id} className={dimmed ? "dimmed" : undefined}>
                <OrchestratorTurn
                  key={message.id}
                  cwd={cwd}
                  message={message}
                  nextUser={
                    messages.slice(i + 1).find((m) => m.role === "user")?.text
                  }
                  tasks={tasks}
                  maxAttempts={settings?.maxAttempts}
                  busy={busy}
                  onOpenTask={onOpenTask}
                  onPick={(option, replyMode) => void onSend(option, replyMode)}
                />
              </div>
            );
          })
        )}
        {busy && (
          <div className="ochat-orch">
            <Avatar />
            <div className="ochat-orch-col">
              <span className="ochat-tool">
                <SquareTerminal size={12} />
                {thread?.note || "Thinking…"}
              </span>
            </div>
          </div>
        )}
        {shownError && (
          <p className="ochat-error" role="alert">
            {shownError}
          </p>
        )}
        <div ref={endRef} />
      </div>
      <div className="ochat-composer">
        <Composer
          value={draft}
          onChange={setDraft}
          onSubmit={submit}
          placeholder={composerPlaceholder(mode, messages)}
          ariaLabel="Message the orchestrator"
          sendLabel="Send message"
          disabled={!thread}
          sending={busy}
          mode={mode}
          onModeChange={onModeChange}
          onStop={() => void orchestratorClient.chatCancel(cwd)}
          route={
            settings && (
              <OrchestratorRouteChip
                settings={settings}
                onRouteChange={onRouteChange}
                standardLabel="Orchestrator"
              />
            )
          }
        />
      </div>
    </>
  );
}

/** The own message turned into an editor: Enter sends, Shift+Enter is a
 * newline, Esc cancels. */
function MessageEditor({
  text,
  onChange,
  onCancel,
  onSend,
}: {
  text: string;
  onChange: (text: string) => void;
  onCancel: () => void;
  onSend: () => void;
}) {
  const areaRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const el = areaRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  useLayoutEffect(() => {
    const el = areaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);
  return (
    <div className="ochat-editor">
      <textarea
        ref={areaRef}
        aria-label="Edit message"
        value={text}
        rows={1}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          const action = editKeyAction({
            key: event.key,
            shiftKey: event.shiftKey,
            isComposing: event.nativeEvent.isComposing,
          });
          if (!action) return;
          event.preventDefault();
          if (action === "cancel") onCancel();
          else onSend();
        }}
      />
      <div className="ochat-editor-bar">
        <span>Replies below will be replaced</span>
        <button
          type="button"
          className="ochat-editor-cancel"
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          type="button"
          className="ochat-editor-send"
          disabled={!canSendEdit(text, false)}
          onClick={onSend}
        >
          Send
        </button>
      </div>
    </div>
  );
}

const KIND_TAG: Record<Message["kind"], { tone: Tone; label: string }> = {
  question: { tone: "warning", label: "question" },
  message: { tone: "info", label: "note" },
  reply: { tone: "ok", label: "reply" },
};

function clock(ts: number) {
  const when = new Date(ts);
  return (
    <time
      className="ochat-card-time"
      dateTime={when.toISOString()}
      title={when.toLocaleString("en-US")}
    >
      {sessionTime(ts)}
    </time>
  );
}

function name(id: string, tasks: Task[]) {
  return id === "orchestrator" ? "orchestrator" : participantLabel(id, tasks);
}

/** One message a task agent sent, with the replies it got under it. A
 * question the orchestrator has not picked up yet takes a reply here. */
function MessageCard({
  message,
  replies,
  tasks,
}: {
  message: Message;
  replies: Message[];
  tasks: Task[];
}) {
  const orchestratorClient = useOrchestratorClient();
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState("");
  const tag = KIND_TAG[message.kind];
  const pending =
    message.kind === "question" &&
    message.to === "orchestrator" &&
    !message.delivered &&
    replies.length === 0;
  function send() {
    const text = draft.trim();
    if (!text) return;
    setSending(true);
    setError("");
    orchestratorClient
      .messageSend({
        from: "orchestrator",
        to: message.from,
        replyTo: message.id,
        text,
      })
      .then(() => setDraft(""))
      .catch((e) => setError(errorText(e)))
      .finally(() => setSending(false));
  }
  return (
    <div className={`ochat-card ${pending ? "pending" : ""}`}>
      <div className="ochat-card-head">
        <Tag tone={tag.tone}>{tag.label}</Tag>
        <span className="ochat-card-from" title={name(message.from, tasks)}>
          {name(message.from, tasks)}
        </span>
        <span className="ochat-card-to">→ {name(message.to, tasks)}</span>
        <span className="ochat-spacer" />
        {clock(message.ts)}
      </div>
      <p className="ochat-card-text">{message.text}</p>
      {replies.map((reply) => (
        <p key={reply.id} className="ochat-card-reply">
          <CornerDownRight size={12} />
          <span>
            <span className="ochat-card-from">{name(reply.from, tasks)}</span>{" "}
            {reply.text}
          </span>
        </p>
      ))}
      {pending && (
        <form
          className="ochat-card-form"
          onSubmit={(event) => {
            event.preventDefault();
            send();
          }}
        >
          <input
            className="ochat-card-input"
            aria-label="Reply as the owner"
            placeholder="Reply as the owner…"
            value={draft}
            disabled={sending}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button
            type="submit"
            className="ui-button secondary"
            disabled={sending || !draft.trim()}
          >
            Reply
          </button>
        </form>
      )}
      {error && (
        <p className="ochat-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** Agent-to-orchestrator (and agent-to-agent) messages the daemon keeps for
 * this repo, newest first, each reply under the message it answers. */
function AgentMessages({ cwd, tasks }: { cwd: string; tasks: Task[] }) {
  const orchestratorClient = useOrchestratorClient();
  const host = useOrchestratorHost();
  const [messages, setMessages] = useState<Message[]>([]);
  const [loadError, setLoadError] = useState("");
  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .messageList(cwd)
      .then((loaded) => !cancelled && setMessages(loaded))
      .catch((e) => !cancelled && setLoadError(errorText(e)));
    const off = window.bridge?.onOrchestrator((event) => {
      if (hostOf(event) !== host) return;
      if (event.event === "message" && event.message.repo === cwd)
        setMessages((old) => upsertMessage(old, event.message));
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [cwd, host, orchestratorClient]);

  const replies = new Map<string, Message[]>();
  for (const m of messages)
    if (m.kind === "reply" && m.replyTo)
      replies.set(m.replyTo, [...(replies.get(m.replyTo) ?? []), m]);
  const known = new Set(messages.map((m) => m.id));
  const top = messages
    .filter((m) => !(m.kind === "reply" && m.replyTo && known.has(m.replyTo)))
    .sort((a, b) => b.ts - a.ts);
  return (
    <>
      <div className="ochat-head ochat-head-messages">
        <h2 className="ochat-head-title">Agent messages</h2>
        <span className="ochat-head-sub">
          what task agents asked or told the orchestrator
        </span>
      </div>
      <div className="ochat-cards">
        {loadError && (
          <p className="ochat-error" role="alert">
            {loadError}
          </p>
        )}
        {top.length === 0 && !loadError ? (
          <p className="ochat-cards-empty">
            No messages yet. Task agents ask the orchestrator here while they
            work.
          </p>
        ) : (
          top.map((message) => (
            <MessageCard
              key={message.id}
              message={message}
              replies={replies.get(message.id) ?? []}
              tasks={tasks}
            />
          ))
        )}
      </div>
    </>
  );
}

/** The right-hand sessions column: New, search, the agent messages entry
 * and this repo's chat sessions, TODAY and EARLIER. A session shows once it
 * has a message: a new chat gets its row on the first send. */
function Sessions({
  list,
  current,
  kind,
  busy,
  pendingMessages,
  previews,
  unread,
  onNew,
  onPick,
  onMessages,
}: {
  list: ChatSessionList | null;
  current: string | undefined;
  /** Last message per session id, for the sessions this view loaded. */
  previews: Record<string, string>;
  unread: (session: ChatSessionSummary) => boolean;
  kind: "chat" | "messages";
  busy: boolean;
  pendingMessages: number;
  onNew: () => void;
  onPick: (id: string) => void;
  onMessages: () => void;
}) {
  const [query, setQuery] = useState("");
  const sessions = (list?.sessions ?? []).filter(
    (session) => session.messageCount > 0 && matchesQuery(query, session.title),
  );
  const wait = busy ? "Wait for the reply to finish" : undefined;
  return (
    <aside className="ochat-sessions" aria-label="Chat sessions">
      <div className="ochat-sessions-top">
        <h2>Chats</h2>
        <button
          type="button"
          className="ui-button secondary"
          disabled={busy}
          title={wait}
          onClick={onNew}
        >
          <Plus size={14} />
          New
        </button>
      </div>
      <label className="ochat-search">
        <Search size={12} />
        <input
          aria-label="Search chats"
          placeholder="Search chats"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <button
        type="button"
        className={`ochat-agent-row ${kind === "messages" ? "selected" : ""}`}
        aria-current={kind === "messages" || undefined}
        onClick={onMessages}
      >
        <MessageSquare size={14} />
        <span>Agent messages</span>
        {pendingMessages > 0 && (
          <span className="ui-count">{pendingMessages}</span>
        )}
      </button>
      {groupSessions(sessions).map((group) => (
        <div key={group.label} className="ochat-group">
          <span className="ochat-group-label">{group.label}</span>
          {group.sessions.map((session) => {
            const selected = kind === "chat" && session.id === current;
            return (
              <button
                key={session.id}
                type="button"
                className={`ochat-session ${selected ? "selected" : ""}`}
                aria-current={selected || undefined}
                disabled={busy && !selected}
                title={busy && !selected ? wait : undefined}
                onClick={() => onPick(session.id)}
              >
                <span className="ochat-session-h">
                  <span className="ochat-session-title">
                    {session.title || "New chat"}
                  </span>
                  {session.busy ? (
                    <span
                      className="ui-dot ui-tone-info"
                      aria-label="Answering"
                    />
                  ) : (
                    !selected &&
                    unread(session) && (
                      <span
                        className="ui-dot ui-tone-info"
                        aria-label="Unread"
                      />
                    )
                  )}
                  <span className="ochat-session-time">
                    {sessionTime(session.updatedAt)}
                  </span>
                </span>
                {previews[session.id] && (
                  <span className="ochat-session-preview">
                    {previews[session.id]}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      ))}
    </aside>
  );
}

/** The Chat view: the conversation (or the agent messages) in the centre and
 * this repo's chat sessions on the right. Stays mounted while hidden so a
 * half-typed message survives a look at a task; becoming visible reloads the
 * current session. `start` (a new `n` each time) opens a new session with a
 * mode selected, e.g. the Plan page's Brainstorm button. */
export function ChatView({
  cwd,
  kind,
  settings,
  tasks,
  pendingMessages,
  onRouteChange,
  start,
  onShow,
  onOpenTask,
}: {
  cwd: string;
  /** Which pane shows; `null` hides the whole view. */
  kind: "chat" | "messages" | null;
  settings: Settings | null;
  tasks: Task[];
  pendingMessages: number;
  onRouteChange: (routeId: string) => void;
  start?: { mode: ChatMode; n: number };
  onShow: (kind: "chat" | "messages") => void;
  onOpenTask?: (id: string) => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const host = useOrchestratorHost();
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [list, setList] = useState<ChatSessionList | null>(null);
  const [mode, setMode] = useState<ChatMode>("chat");
  // A mode asked for before its session exists (chatNew is still on its way).
  const wanted = useRef<ChatMode | null>(null);
  const [error, setError] = useState("");
  const visible = kind !== null;
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [seen, setSeen] = useState<Record<string, number>>({});
  const [since] = useState(() => Date.now());

  useEffect(() => {
    if (!thread) return;
    const text = lastMessageText(thread);
    setPreviews((old) =>
      old[thread.id] === text ? old : { ...old, [thread.id]: text },
    );
  }, [thread]);
  useEffect(() => {
    if (thread && kind === "chat")
      setSeen((old) => ({ ...old, [thread.id]: Date.now() }));
  }, [thread, kind]);

  useEffect(() => {
    const off = window.bridge?.onOrchestrator((event) => {
      if (
        hostOf(event) !== host ||
        event.event !== "chat" ||
        event.thread.repo !== cwd
      )
        return;
      setList({ current: event.current, sessions: event.sessions });
      if (!event.thread.busy) setError("");
      setThread(event.thread);
    });
    return () => off?.();
  }, [cwd, host]);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    Promise.all([
      orchestratorClient.chatGet(cwd),
      orchestratorClient.chatList(cwd),
    ])
      .then(([loaded, sessions]) => {
        if (cancelled) return;
        setThread(loaded);
        setList(sessions);
        setError("");
      })
      .catch((e) => !cancelled && setError(errorText(e)));
    return () => {
      cancelled = true;
    };
  }, [cwd, visible, orchestratorClient]);

  // The composer restores the session's last mode when it is opened.
  const threadId = thread?.id;
  const threadMode = thread?.mode;
  useEffect(() => {
    setMode(wanted.current ?? threadMode ?? "chat");
    wanted.current = null;
    // Only a different session changes the mode; the mode of a reply that
    // arrives in the same session is the one already selected.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId]);
  const startedAt = useRef(start?.n ?? 0);
  useEffect(() => {
    if (!start || start.n === startedAt.current) return;
    startedAt.current = start.n;
    if (thread && thread.messages.length === 0 && !thread.busy) {
      setMode(start.mode);
      return;
    }
    wanted.current = start.mode;
    change(orchestratorClient.chatNew(cwd));
    // `start` is the trigger; the thread it acts on is the current one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [start?.n]);
  function change(request: Promise<ChatThread>) {
    setError("");
    request
      .then((next) => {
        setThread(next);
        onShow("chat");
      })
      .catch((e) => setError(errorText(e)));
  }
  function send(text: string, sendMode: ChatMode): Promise<boolean> {
    setError("");
    return orchestratorClient
      .chatSend(cwd, text, sendMode)
      .then(() => true)
      .catch((e) => {
        setError(errorText(e));
        return false;
      });
  }
  function edit(messageId: string, text: string, editMode: ChatMode) {
    setError("");
    return orchestratorClient
      .chatEdit(cwd, messageId, text, editMode)
      .then(() => true)
      .catch((e) => {
        setError(errorText(e));
        return false;
      });
  }
  const busy = Boolean(list?.sessions.some((s) => s.busy) || thread?.busy);
  return (
    <div className="orch-chat-view ochat" hidden={!visible}>
      <section className="ochat-convo" hidden={kind === "messages"}>
        <Conversation
          cwd={cwd}
          thread={thread}
          error={error}
          settings={settings}
          tasks={tasks}
          mode={mode}
          onModeChange={setMode}
          onRouteChange={onRouteChange}
          onOpenTask={onOpenTask}
          onSend={send}
          onEdit={edit}
          onClear={() => change(orchestratorClient.chatClear(cwd))}
        />
      </section>
      <section className="ochat-convo" hidden={kind !== "messages"}>
        <AgentMessages cwd={cwd} tasks={tasks} />
      </section>
      <Sessions
        list={list}
        current={thread?.id}
        kind={kind ?? "chat"}
        busy={busy}
        pendingMessages={pendingMessages}
        previews={previews}
        unread={(session) => isUnread(session, seen[session.id], since)}
        onNew={() =>
          thread && thread.messages.length === 0 && !thread.busy
            ? onShow("chat")
            : change(orchestratorClient.chatNew(cwd))
        }
        onPick={(id) =>
          id === thread?.id
            ? onShow("chat")
            : change(orchestratorClient.chatSwitch(cwd, id))
        }
        onMessages={() => onShow("messages")}
      />
    </div>
  );
}
