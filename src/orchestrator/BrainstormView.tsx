import { useEffect, useRef, useState } from "react";
import { ChevronDown, Plus, Sparkles } from "lucide-react";
import { enterKind, eventKind, type KindedList } from "./chatKinds";
import { newestFirst, sessionTime } from "./chatModel";
import { errorText } from "./helpers";
import { useOrchestratorClient, useOrchestratorHost } from "./hostContext";
import { hostOf } from "./hosts";
import { draftCreateParams } from "./planModel";
import type { ChatDraft, ChatMessage, ChatThread, Task } from "./types";
import { Chip, Criterion, Tag } from "./ui";
import { Composer } from "./Composer";
import "./plan.css";
import "./chat.css";

const STARTERS = [
  "Run tasks on my lab server",
  "Answer questions from the Inbox",
  "A weekly digest of what landed",
];

function Bubble({
  message,
  nextUser,
  disabled,
  onPick,
}: {
  message: ChatMessage;
  /** The owner's next message, to mark the option they picked. */
  nextUser: string | undefined;
  disabled: boolean;
  onPick: (option: string) => void;
}) {
  const you = message.role === "user";
  // orchd falls back to the question as the reply's text when the reply was
  // only a block; that question is not shown twice.
  const questions = (message.questions ?? []).filter(
    (q) => q.text !== message.text || q.options.length > 0,
  );
  return (
    <div className={`brainstorm-bubble ${you ? "you" : "orch"}`}>
      <span className="brainstorm-who">{you ? "You" : "Orchestrator"}</span>
      <p className="brainstorm-text">{message.text}</p>
      {questions.map((question) => (
        <div key={question.text} className="brainstorm-question">
          {question.text !== message.text && (
            <p className="brainstorm-text">{question.text}</p>
          )}
          {question.options.length > 0 && (
            <div className="brainstorm-opts">
              {question.options.map((option) => (
                <Chip
                  key={option}
                  selected={nextUser === option}
                  disabled={disabled}
                  onClick={() => onPick(option)}
                >
                  {option}
                </Chip>
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

function DraftPanel({
  draft,
  tasks,
  busy,
  onAdd,
  onStart,
}: {
  draft: ChatDraft | undefined;
  tasks: Task[];
  busy: boolean;
  onAdd: () => void;
  onStart: () => void;
}) {
  const titles = (draft?.dependsOn ?? []).map(
    (ref) => tasks.find((t) => t.id === ref)?.title ?? ref,
  );
  return (
    <aside className="draft-panel ochat-draft" aria-label="Draft task">
      <div className="draft-panel-head">
        <span className="draft-eyebrow">DRAFT TASK</span>
        <span className="draft-fills">fills as you talk</span>
      </div>
      {draft ? (
        <>
          <h3 className="draft-title">{draft.title}</h3>
          {draft.goal && <p className="draft-goal">{draft.goal}</p>}
          {draft.criteria.length > 0 && (
            <>
              <span className="draft-eyebrow">ACCEPTANCE</span>
              <div className="draft-criteria">
                {draft.criteria.map((criterion) => (
                  <Criterion key={criterion} state="pending">
                    {criterion}
                  </Criterion>
                ))}
              </div>
            </>
          )}
          <dl className="draft-meta">
            <div>
              <dt>Depends on</dt>
              <dd>{titles.length ? titles.join(", ") : "—"}</dd>
            </div>
            <div>
              <dt>Tier</dt>
              <dd>{draft.tier ? `${draft.tier} (guess)` : "—"}</dd>
            </div>
          </dl>
          <span className="draft-spacer" />
          <div className="ochat-draft-acts">
            <button
              type="button"
              className="ui-button secondary"
              disabled={busy}
              onClick={onAdd}
            >
              Add to plan
            </button>
            <button
              type="button"
              className="ui-button primary"
              disabled={busy}
              onClick={onStart}
            >
              Start now
            </button>
          </div>
        </>
      ) : (
        <p className="ochat-draft-empty">
          The draft fills as you talk: title, goal, acceptance, dependencies and
          a tier guess.
        </p>
      )}
    </aside>
  );
}

/** "New brainstorm ▾": start a fresh brainstorm or reopen an earlier one. */
function BrainstormMenu({
  list,
  current,
  disabled,
  onNew,
  onPick,
}: {
  list: KindedList | null;
  current: string | undefined;
  disabled: boolean;
  onNew: () => void;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const earlier = (list?.sessions ?? [])
    .filter((s) => s.id !== current && s.messageCount > 0)
    .sort(newestFirst);
  return (
    <div className="ochat-bs-menu-wrap">
      <button
        type="button"
        className="ochat-bs-menu-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        title={disabled ? "Wait for the reply to finish" : undefined}
        onClick={() => setOpen((v) => !v)}
      >
        New brainstorm
        <ChevronDown size={12} />
      </button>
      {open && (
        <div className="ochat-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onNew();
            }}
          >
            <Plus size={13} /> New brainstorm
          </button>
          {earlier.map((session) => (
            <button
              key={session.id}
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onPick(session.id);
              }}
            >
              <span className="ochat-menu-label">
                {session.title || "Brainstorm"}
              </span>
              <span className="ochat-session-time">
                {sessionTime(session.updatedAt)}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The Brainstorm view: a conversation on orchd's brainstorm sessions
 * (never the Chat view's) and the task draft the session keeps. Opening it
 * shows the current brainstorm; orchd starts an empty one when there is
 * none. */
export function BrainstormView({
  cwd,
  tasks = [],
  onCreated,
  onOpenTask,
  onPlan,
}: {
  cwd?: string;
  tasks?: Task[];
  onCreated?: (task: Task) => void;
  onOpenTask?: (id: string) => void;
  onPlan?: () => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const host = useOrchestratorHost();
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [list, setList] = useState<KindedList | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // The session whose draft was just made into a task: its draft stays hidden
  // until the next chat event, even when orchd refused to clear it.
  const [usedDraft, setUsedDraft] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!cwd) return;
    const off = window.bridge?.onOrchestrator((event) => {
      if (
        hostOf(event) !== host ||
        event.event !== "chat" ||
        eventKind(event) !== "brainstorm" ||
        event.thread.repo !== cwd
      )
        return;
      setList({ current: event.current, sessions: event.sessions });
      setThread(event.thread);
      setUsedDraft(null);
    });
    return () => off?.();
  }, [cwd, host]);

  useEffect(() => {
    if (!cwd) return;
    let cancelled = false;
    enterKind(cwd, "brainstorm", orchestratorClient)
      .then((entered) => {
        if (cancelled) return;
        setThread(entered.thread);
        setList(entered.list);
      })
      .catch((e) => !cancelled && setError(errorText(e)));
    return () => {
      cancelled = true;
    };
  }, [cwd, orchestratorClient]);

  const messages = thread?.messages ?? [];
  const last = messages.length ? messages[messages.length - 1].id : "";
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [last]);

  function send(body: string) {
    if (!cwd) return Promise.resolve(false);
    setError("");
    return orchestratorClient
      .chatSend(cwd, body, "brainstorm")
      .then(() => true)
      .catch((e) => {
        setError(errorText(e));
        return false;
      });
  }
  function submit() {
    const body = text.trim();
    if (!body || thread?.busy) return;
    void send(body).then((sent) => sent && setText(""));
  }
  function change(request: Promise<ChatThread>) {
    setError("");
    request.then(setThread).catch((e) => setError(errorText(e)));
  }
  const draft = thread && usedDraft !== thread.id ? thread.draft : undefined;
  const chatBusy = Boolean(thread?.busy);

  /** Creates the task, then drops the draft from the session it came from:
   * it has been acted on. Not while a reply streams - orchd refuses to clear
   * the draft then, and a second click would create the task again. A draft
   * orchd would not clear stays hidden here, and the view stays open to say
   * so instead of moving on. */
  async function create(start: boolean) {
    if (!cwd || !thread || !draft || busy || chatBusy) return;
    const session = thread.id;
    setBusy(true);
    setError("");
    try {
      const task = await orchestratorClient.taskCreate(
        cwd,
        draftCreateParams(draft, tasks, start),
      );
      setUsedDraft(session);
      onCreated?.(task);
      try {
        const cleared = await orchestratorClient.chatClearDraft(
          cwd,
          "brainstorm",
          session,
        );
        if (cleared.id === session) setThread(cleared);
      } catch (e) {
        setError(
          `“${task.title}” was created, but its draft could not be cleared: ${errorText(e)}`,
        );
        return;
      }
      if (start) onOpenTask?.(task.id);
      else onPlan?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  const anyBusy = chatBusy || !!list?.sessions.some((s) => s.busy);
  const title = draft?.title || thread?.title;
  const menu = cwd && (
    <BrainstormMenu
      list={list}
      current={thread?.id}
      disabled={anyBusy}
      onNew={() => {
        if (!thread || thread.messages.length > 0)
          change(orchestratorClient.chatNew(cwd, "brainstorm"));
      }}
      onPick={(id) =>
        change(orchestratorClient.chatSwitch(cwd, id, "brainstorm"))
      }
    />
  );
  const empty = messages.length === 0 && !chatBusy;
  const shownError = error || thread?.error;
  return (
    <div className="brainstorm ochat-bs">
      <div className="ochat-bs-main">
        <div className={`brainstorm-convo ${empty ? "ochat-bs-empty" : ""}`}>
          {empty ? (
            <div className="ochat-empty">
              {menu}
              <h3>What do you want to build?</h3>
              <p>
                Describe a feature in a sentence. I ask one question at a time
                and fill the draft on the right; nothing runs until you add it
                to the plan.
              </p>
              <div className="ochat-starters">
                {STARTERS.map((starter) => (
                  <Chip
                    key={starter}
                    disabled={!thread}
                    onClick={() => void send(starter)}
                  >
                    {starter}
                  </Chip>
                ))}
              </div>
            </div>
          ) : (
            <>
              <div className="brainstorm-head">
                <Tag tone="info" dot={false}>
                  Brainstorm
                </Tag>
                {title && (
                  <span className="brainstorm-head-title">{title}</span>
                )}
                <span className="ochat-spacer" />
                {menu}
              </div>
              {messages.map((message, i) => (
                <Bubble
                  key={message.id}
                  message={message}
                  nextUser={
                    messages.slice(i + 1).find((m) => m.role === "user")?.text
                  }
                  disabled={chatBusy || !cwd}
                  onPick={(option) => void send(option)}
                />
              ))}
              {chatBusy && (
                <p className="ochat-tool ochat-bs-note">
                  {thread?.note || "Thinking…"}
                </p>
              )}
            </>
          )}
          {shownError && (
            <p className="brainstorm-error" role="alert">
              {shownError}
            </p>
          )}
          <div ref={endRef} />
        </div>
        <div className="brainstorm-composer">
          <Composer
            value={text}
            onChange={setText}
            onSubmit={submit}
            placeholder={
              empty ? "Describe the feature…" : "Answer, or add a detail…"
            }
            ariaLabel="Brainstorm message"
            sendLabel="Send message"
            disabled={!cwd || !thread}
            sending={chatBusy}
            onStop={() =>
              cwd && void orchestratorClient.chatCancel(cwd, "brainstorm")
            }
            route={
              <span className="chip orch-composer-route-static">
                <Sparkles size={14} className="orch-composer-route-icon" />
                <span className="orch-composer-mode">Brainstorm</span>
                <ChevronDown size={12} />
              </span>
            }
          />
        </div>
      </div>
      <DraftPanel
        draft={draft}
        tasks={tasks}
        busy={busy || chatBusy}
        onAdd={() => void create(false)}
        onStart={() => void create(true)}
      />
    </div>
  );
}
