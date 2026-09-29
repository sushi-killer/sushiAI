import { useEffect, useState } from "react";
import { CornerDownRight, MessageSquare, Plus, Trash2 } from "lucide-react";
import { orchestratorClient } from "./client";
import {
  errorText,
  messageThreads,
  participantLabel,
  upsertMessage,
  type MessageThread,
} from "./helpers";
import type {
  ChatSessionList,
  ChatThread,
  Message,
  Settings,
  Task,
} from "./types";
import { ChatTranscript } from "../ChatTranscript";
import { RichText } from "../agents/AgentsView";
import { Composer, OrchestratorRouteChip } from "./Composer";

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
      <Composer
        value={draft}
        onChange={setDraft}
        onSubmit={submit}
        placeholder="Ask the orchestrator…"
        ariaLabel="Message the orchestrator"
        sendLabel="Send message"
        sending={busy}
        onStop={() => void orchestratorClient.chatCancel(cwd)}
        route={
          settings && (
            <OrchestratorRouteChip
              settings={settings}
              onRouteChange={onRouteChange}
            />
          )
        }
      />
    </div>
  );
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

/** The Chat view: the orchestrator chat and, one segment over, the agent
 * messages the daemon keeps. Both stay mounted while hidden so a half-typed
 * message and the scroll position survive a look at a task. */
export function ChatView({
  cwd,
  kind,
  settings,
  tasks,
  pendingMessages,
  onRouteChange,
  onShow,
}: {
  cwd: string;
  /** Which segment shows; `null` hides the whole view. */
  kind: "chat" | "messages" | null;
  settings: Settings | null;
  tasks: Task[];
  pendingMessages: number;
  onRouteChange: (routeId: string) => void;
  onShow: (kind: "chat" | "messages") => void;
}) {
  return (
    <div className="orch-chat-view" hidden={kind === null}>
      <div className="orch-chat-tabs" role="tablist" aria-label="Chat">
        <button
          role="tab"
          aria-selected={kind === "chat"}
          className={kind === "chat" ? "selected" : ""}
          onClick={() => onShow("chat")}
        >
          Chat
        </button>
        <button
          role="tab"
          aria-selected={kind === "messages"}
          className={kind === "messages" ? "selected" : ""}
          onClick={() => onShow("messages")}
        >
          Messages
          {pendingMessages > 0 && (
            <span className="ui-count">{pendingMessages}</span>
          )}
        </button>
      </div>
      <OrchestratorChat
        cwd={cwd}
        hidden={kind !== "chat"}
        settings={settings}
        onRouteChange={onRouteChange}
      />
      <MessagesView cwd={cwd} tasks={tasks} hidden={kind !== "messages"} />
    </div>
  );
}
