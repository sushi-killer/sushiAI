import { useEffect, useRef, useState } from "react";
import { ChevronDown, Plus, Sparkles } from "lucide-react";
import { orchestratorClient } from "./client";
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
import { optionsInText, sessionTime, stripOptions } from "./chatModel";
import { errorText } from "./helpers";
import {
  DRAFT_REQUEST,
  draftCreateParams,
  draftOfThread,
  optionsOf,
  stripDraftBlock,
  type DraftTask,
} from "./planModel";
import type { ChatMessage, ChatThread, Task } from "./types";
import { Chip, Criterion, Tag } from "./ui";
import { Composer } from "./Composer";
import "./plan.css";
import "./chat.css";

const STARTERS = [
  "Run tasks on my lab server",
  "Answer questions from the Inbox",
  "A weekly digest of what landed",
];

function createDraftTask(
  cwd: string,
  draft: DraftTask,
  tasks: Task[],
  start: boolean,
): Promise<Task> {
  if (!window.bridge)
    return Promise.reject(new Error("Open the desktop app first."));
  return window.bridge.orchestrator("task.create", {
    repo: cwd,
    ...draftCreateParams(draft, tasks, start),
  }) as Promise<Task>;
}

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
  const text = you
    ? message.text === DRAFT_REQUEST
      ? "Draft this as a task."
      : visibleText(message.text)
    : stripOptions(stripDraftBlock(message.text)) || "Draft updated.";
  const options = you
    ? []
    : [...new Set([...optionsOf(message), ...optionsInText(message.text)])];
  return (
    <div className={`brainstorm-bubble ${you ? "you" : "orch"}`}>
      <span className="brainstorm-who">{you ? "You" : "Orchestrator"}</span>
      <p className="brainstorm-text">{text}</p>
      {options.length > 0 && (
        <div className="brainstorm-opts">
          {options.map((option) => (
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
  );
}

function DraftPanel({
  draft,
  tasks,
  busy,
  drafting,
  canDraft,
  onDraft,
  onAdd,
  onStart,
}: {
  draft: DraftTask | null;
  tasks: Task[];
  busy: boolean;
  drafting: boolean;
  canDraft: boolean;
  onDraft: () => void;
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
                {draft.criteria.map((c) => (
                  <Criterion key={c.text} state={c.met ? "met" : "pending"}>
                    {c.text}
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
            <div>
              <dt>Base</dt>
              <dd>{draft.base ?? "current branch"}</dd>
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
        <>
          <p className="ochat-draft-empty">
            The draft fills as you talk: title, goal, acceptance, dependencies
            and a tier guess.
          </p>
          {canDraft && (
            <button
              type="button"
              className="ui-button ghost brainstorm-draft-btn"
              disabled={drafting}
              onClick={onDraft}
            >
              <Sparkles size={14} />
              {drafting ? "Drafting…" : "Draft from this chat"}
            </button>
          )}
        </>
      )}
    </aside>
  );
}

/** "New brainstorm ▾": start a fresh brainstorm or reopen an earlier one. */
function BrainstormMenu({
  cwd,
  list,
  current,
  disabled,
  onNew,
  onPick,
}: {
  cwd: string;
  list: KindedList | null;
  current: string | undefined;
  disabled: boolean;
  onNew: () => void;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const earlier = sessionsOfKind(cwd, list, "brainstorm")
    .filter((s) => s.id !== current && s.title)
    .reverse();
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
          {earlier.map((session) => {
            const { time } = sessionMeta(cwd, session);
            return (
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
                  {visibleTitle(session.title)}
                </span>
                {time !== undefined && (
                  <span className="ochat-session-time">
                    {sessionTime(time)}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** The Brainstorm view: a conversation on brainstorm sessions of its own
 * (never the Chat view's), and the draft task it fills. Opening it makes the
 * last brainstorm current, or starts one. A structured `draft` on the thread
 * (orchd) drives the panel; until then the orchestrator's `sushi-plan`
 * blocks do. */
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
  const [thread, setThread] = useState<ChatThread | null>(null);
  const [list, setList] = useState<KindedList | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [drafting, setDrafting] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [retry, setRetry] = useState(0);
  const waitingRef = useRef(false);
  waitingRef.current = waiting;
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!cwd) return;
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event !== "chat" || event.thread.repo !== cwd) return;
      const next = { current: event.current, sessions: event.sessions };
      setList(next);
      noteThread(cwd, event.thread);
      if (isOfKind(cwd, next, event.thread.id, "brainstorm")) {
        if (!event.thread.busy) setDrafting(false);
        setThread(event.thread);
      }
      if (!event.thread.busy && waitingRef.current) setRetry((n) => n + 1);
    });
    return () => off?.();
  }, [cwd]);

  useEffect(() => {
    if (!cwd) return;
    let cancelled = false;
    enterKind(cwd, "brainstorm")
      .then((entered) => {
        if (cancelled) return;
        setThread(entered.thread);
        setList(entered.list);
        setWaiting(false);
      })
      .catch((e) => {
        if (cancelled) return;
        const message = errorText(e);
        if (/still answering/i.test(message)) setWaiting(true);
        else setError(message);
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, retry]);

  const shown = waiting ? null : thread;
  const messages = shown?.messages ?? [];
  const last = messages.length ? messages[messages.length - 1].id : "";
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [last]);

  function send(body: string) {
    if (!cwd) return Promise.resolve(false);
    setError("");
    return sendInKind(cwd, "brainstorm", body, shown, list)
      .then(() => true)
      .catch((e) => {
        setError(errorText(e));
        return false;
      });
  }
  function submit() {
    const body = text.trim();
    if (!body || shown?.busy) return;
    void send(body).then((sent) => sent && setText(""));
  }
  function requestDraft() {
    setDrafting(true);
    void send(DRAFT_REQUEST).then((sent) => sent || setDrafting(false));
  }
  function change(request: Promise<ChatThread>) {
    setError("");
    request.then(setThread).catch((e) => setError(errorText(e)));
  }
  const draft = shown ? draftOfThread(shown) : null;

  function create(start: boolean) {
    if (!cwd || !draft) return;
    setBusy(true);
    setError("");
    createDraftTask(cwd, draft, tasks, start)
      .then((task) => {
        onCreated?.(task);
        if (start) onOpenTask?.(task.id);
        else onPlan?.();
      })
      .catch((e) => setError(errorText(e)))
      .finally(() => setBusy(false));
  }

  const chatBusy = Boolean(shown?.busy);
  const anyBusy = chatBusy || !!list?.sessions.some((s) => s.busy);
  const title = draft?.title || visibleTitle(shown?.title);
  const menu = cwd && (
    <BrainstormMenu
      cwd={cwd}
      list={list}
      current={shown?.id}
      disabled={anyBusy || waiting}
      onNew={() => {
        if (!shown || shown.messages.length > 0)
          change(newSession(cwd, "brainstorm"));
      }}
      onPick={(id) => change(switchSession(cwd, "brainstorm", id))}
    />
  );
  const empty = messages.length === 0 && !chatBusy;
  const shownError = error || shown?.error;
  return (
    <div className="brainstorm ochat-bs">
      <div className="ochat-bs-main">
        <div className={`brainstorm-convo ${empty ? "ochat-bs-empty" : ""}`}>
          {waiting && (
            <p className="ochat-wait">
              The orchestrator is still answering in another session. The
              brainstorm opens when it finishes.
            </p>
          )}
          {empty ? (
            !waiting && (
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
                      disabled={!shown}
                      onClick={() => void send(starter)}
                    >
                      {starter}
                    </Chip>
                  ))}
                </div>
              </div>
            )
          ) : (
            <>
              <div className="brainstorm-head">
                <Tag tone="info">Brainstorm</Tag>
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
                    messages
                      .slice(i + 1)
                      .find((m) => m.role === "user")
                      ?.text.split("\n\n")[0]
                  }
                  disabled={chatBusy || !cwd}
                  onPick={(option) => void send(option)}
                />
              ))}
              {chatBusy && (
                <p className="ochat-tool ochat-bs-note">
                  {shown?.note || "Thinking…"}
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
            disabled={!cwd || !shown}
            sending={chatBusy}
            onStop={() => cwd && void orchestratorClient.chatCancel(cwd)}
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
        busy={busy}
        drafting={drafting || chatBusy}
        canDraft={messages.length > 0}
        onDraft={requestDraft}
        onAdd={() => create(false)}
        onStart={() => create(true)}
      />
    </div>
  );
}
