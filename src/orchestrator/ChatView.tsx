import { useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  CornerDownRight,
  MessageSquare,
  MoreHorizontal,
  Plus,
  Search,
  Sparkles,
  SquareTerminal,
} from "lucide-react";
import { useOrchestratorClient } from "./hostContext";
import {
  enterKind,
  isOfKind,
  newSession,
  noteThread,
  sendInKind,
  sessionMeta,
  sessionsOfKind,
  switchSession,
  visibleText,
  visibleTitle,
  type KindedList,
} from "./chatKinds";
import {
  groupSessions,
  matchesQuery,
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
import type { ChatMessage, ChatThread, Message, Settings, Task } from "./types";
import { RichText } from "../agents/AgentsView";
import { Composer, OrchestratorRouteChip } from "./Composer";
import { Chip, Tag } from "./ui";
import "./chat.css";

const STARTERS = [
  "What is running and what needs me?",
  "Why did the last task fail?",
  "Land everything that passed review",
];

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
  message,
  tasks,
  maxAttempts,
  onOpenTask,
}: {
  message: ChatMessage;
  tasks: Task[];
  maxAttempts?: number;
  onOpenTask?: (id: string) => void;
}) {
  const refs = taskRefs(message.text, tasks);
  return (
    <div className="ochat-orch">
      <Avatar />
      <div className="ochat-orch-col">
        <div className="ochat-orch-text">
          <RichText text={message.text} />
        </div>
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
  waiting,
  error,
  settings,
  tasks,
  onRouteChange,
  onOpenTask,
  onSend,
  onClear,
}: {
  cwd: string;
  thread: ChatThread | null;
  waiting: boolean;
  error: string;
  settings: Settings | null;
  tasks: Task[];
  onRouteChange: (routeId: string) => void;
  onOpenTask?: (id: string) => void;
  onSend: (text: string) => Promise<boolean>;
  onClear: () => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [draft, setDraft] = useState("");
  const endRef = useRef<HTMLDivElement>(null);
  const messages = thread?.messages ?? [];
  const busy = Boolean(thread?.busy);
  const last = messages.length ? messages[messages.length - 1].id : "";
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [last, busy]);

  function submit() {
    const text = draft.trim();
    if (!text || busy) return;
    void onSend(text).then((sent) => sent && setDraft(""));
  }
  const shownError = error || thread?.error;
  const empty = messages.length === 0 && !busy;
  return (
    <>
      <ChatHead
        title={visibleTitle(thread?.title) || "New chat"}
        settings={settings}
        busy={busy}
        canClear={messages.length > 0}
        onClear={onClear}
      />
      <div className={`ochat-scroll ${empty ? "empty" : ""}`}>
        {waiting && (
          <p className="ochat-wait">
            The orchestrator is still answering in another session. This chat
            opens when it finishes.
          </p>
        )}
        {empty && !waiting ? (
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
                  onClick={() => void onSend(text)}
                >
                  {text}
                </Chip>
              ))}
            </div>
          </div>
        ) : (
          messages.map((message) =>
            message.role === "user" ? (
              <div key={message.id} className="ochat-you">
                <div className="ochat-bubble">{visibleText(message.text)}</div>
              </div>
            ) : (
              <OrchestratorTurn
                key={message.id}
                message={message}
                tasks={tasks}
                maxAttempts={settings?.maxAttempts}
                onOpenTask={onOpenTask}
              />
            ),
          )
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
          placeholder="Ask the orchestrator…"
          ariaLabel="Message the orchestrator"
          sendLabel="Send message"
          disabled={!thread || waiting}
          sending={busy}
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
  }, [cwd, orchestratorClient]);

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
 * and this repo's chat sessions, TODAY and EARLIER. */
function Sessions({
  cwd,
  list,
  current,
  kind,
  busy,
  pendingMessages,
  onNew,
  onPick,
  onMessages,
}: {
  cwd: string;
  list: KindedList | null;
  current: string | undefined;
  kind: "chat" | "messages";
  busy: boolean;
  pendingMessages: number;
  onNew: () => void;
  onPick: (id: string) => void;
  onMessages: () => void;
}) {
  const [query, setQuery] = useState("");
  const rows = sessionsOfKind(cwd, list, "chat")
    .map((session) => ({ session, ...sessionMeta(cwd, session) }))
    .filter((row) =>
      matchesQuery(query, visibleTitle(row.session.title), row.preview),
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
      {groupSessions(rows).map((group) => (
        <div key={group.label} className="ochat-group">
          <span className="ochat-group-label">{group.label}</span>
          {group.rows.map(({ session, time, preview }) => {
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
                    {visibleTitle(session.title) || "New chat"}
                  </span>
                  {session.busy && (
                    <span
                      className="ui-dot ui-tone-info"
                      aria-label="Answering"
                    />
                  )}
                  {time !== undefined && (
                    <span className="ochat-session-time">
                      {sessionTime(time)}
                    </span>
                  )}
                </span>
                {(preview || !session.title) && (
                  <span className="ochat-session-preview">
                    {preview || "No messages yet"}
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
 * half-typed message survives a look at a task; becoming visible makes the
 * last chat session current again (Brainstorm keeps sessions of its own). */
export function ChatView({
  cwd,
  kind,
  settings,
  tasks,
  pendingMessages,
  onRouteChange,
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
  onShow: (kind: "chat" | "messages") => void;
  onOpenTask?: (id: string) => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [list, setList] = useState<KindedList | null>(null);
  const [error, setError] = useState("");
  const [waiting, setWaiting] = useState(false);
  const [retry, setRetry] = useState(0);
  const waitingRef = useRef(false);
  waitingRef.current = waiting;
  const visible = kind !== null;

  useEffect(() => {
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event !== "chat" || event.thread.repo !== cwd) return;
      const next = { current: event.current, sessions: event.sessions };
      setList(next);
      noteThread(cwd, event.thread);
      if (isOfKind(cwd, next, event.thread.id, "chat")) {
        if (!event.thread.busy) setError("");
        setThread(event.thread);
      }
      if (!event.thread.busy && waitingRef.current) setRetry((n) => n + 1);
    });
    return () => off?.();
  }, [cwd]);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    enterKind(cwd, "chat")
      .then((entered) => {
        if (cancelled) return;
        setThread(entered.thread);
        setList(entered.list);
        setWaiting(false);
        setError("");
      })
      .catch((e) => {
        if (cancelled) return;
        const text = errorText(e);
        if (/still answering/i.test(text)) setWaiting(true);
        else setError(text);
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, visible, retry]);

  function change(request: Promise<ChatThread>) {
    setError("");
    request
      .then((next) => {
        setThread(next);
        onShow("chat");
      })
      .catch((e) => setError(errorText(e)));
  }
  function send(text: string): Promise<boolean> {
    setError("");
    return sendInKind(cwd, "chat", text, thread, list)
      .then(() => true)
      .catch((e) => {
        setError(errorText(e));
        return false;
      });
  }
  const busy = Boolean(list?.sessions.some((s) => s.busy) || thread?.busy);
  const shown = waiting ? null : thread;
  return (
    <div className="orch-chat-view ochat" hidden={!visible}>
      <section className="ochat-convo" hidden={kind === "messages"}>
        <Conversation
          cwd={cwd}
          thread={shown}
          waiting={waiting}
          error={error}
          settings={settings}
          tasks={tasks}
          onRouteChange={onRouteChange}
          onOpenTask={onOpenTask}
          onSend={send}
          onClear={() => change(orchestratorClient.chatClear(cwd))}
        />
      </section>
      <section className="ochat-convo" hidden={kind !== "messages"}>
        <AgentMessages cwd={cwd} tasks={tasks} />
      </section>
      <Sessions
        cwd={cwd}
        list={list}
        current={shown?.id}
        kind={kind ?? "chat"}
        busy={busy}
        pendingMessages={pendingMessages}
        onNew={() =>
          shown && shown.messages.length === 0 && !shown.busy
            ? onShow("chat")
            : change(newSession(cwd, "chat"))
        }
        onPick={(id) =>
          id === shown?.id
            ? onShow("chat")
            : change(switchSession(cwd, "chat", id))
        }
        onMessages={() => onShow("messages")}
      />
    </div>
  );
}
