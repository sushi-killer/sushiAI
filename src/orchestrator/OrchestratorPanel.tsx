import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArchiveRestore,
  ChartColumn,
  ChevronDown,
  GitBranch,
  LayoutList,
  ListTodo,
  MessageSquare,
  Sparkles,
  X,
} from "lucide-react";
import "./orchestrator.css";
import type { OrchestratorView } from "../types";
import type { TaskTarget } from "./notices";
import { pendingReveal, subscribeReveal } from "./reveal";
import { orchestratorClient } from "./client";
import {
  applyOrchestratorEvent,
  emptyLiveState,
  daemonDown,
  errorText,
  formatDuration,
  planDrafts,
  sortTasks,
  taskCreateParams,
  taskTone,
  unreadChatCount,
  upsertMessage,
  upsertTask,
  type OrchestratorLiveState,
} from "./helpers";
import { needsOwner } from "./ownerAttention";
import { upsertProposal } from "./improvementsModel";
import type { ChatMessage, Message, Proposal, Settings, Task } from "./types";
import { Banner, Tag } from "./ui";
import { OrchRail } from "./OrchRail";
import {
  enterKind,
  isOfKind,
  listSessions,
  type KindedList,
} from "./chatKinds";
import { Composer, OrchestratorRouteChip, TaskRouteLabel } from "./Composer";
import { TaskDetail } from "./TaskDetail";
import { HomeView } from "./HomeView";
import { ChatView } from "./ChatView";
import { ImprovementsView } from "./Improvements";
import { AnalyticsView } from "./Analytics";
import { PlanView } from "./PlanView";
import { BrainstormView } from "./BrainstormView";

type DaemonState = "loading" | "ready" | "not-built" | "unavailable";

/** What the main pane beside the rail shows: see `OrchestratorView`. */
type View = OrchestratorView;

/** The task composer creates a task directly through `task.create`, not a
 * chat round-trip with the orchestrator agent. It prefers the `request`
 * form (the planner drafts title/goal/criteria/verify), but a disabled
 * planner shouldn't dead-end the control - falling back to the plain
 * title/goal form keeps it working either way. `start: false` keeps it as a
 * draft on the Plan. */
async function createTask(
  cwd: string,
  text: string,
  base: string,
  start: boolean,
): Promise<Task> {
  try {
    return await orchestratorClient.taskCreate(
      cwd,
      taskCreateParams({ request: text, start, source: "ui" }, base),
    );
  } catch (e) {
    if (!errorText(e).includes("planner is disabled")) throw e;
    return orchestratorClient.taskCreate(
      cwd,
      taskCreateParams(
        {
          title: text.length > 60 ? `${text.slice(0, 59)}…` : text,
          goal: text,
          start,
          source: "ui",
        },
        base,
      ),
    );
  }
}

/** When the owner last looked at this repo's orchestrator chat, kept in
 * this browser only; the key is per repo. */
function chatSeenKey(cwd: string): string {
  return `sushiai.orchestrator.chatSeen:${cwd}`;
}
function readChatSeen(cwd: string): number | null {
  try {
    const raw = window.localStorage.getItem(chatSeenKey(cwd));
    return raw === null ? null : Number(raw) || 0;
  } catch {
    return null;
  }
}
function writeChatSeen(cwd: string, ts: number) {
  try {
    window.localStorage.setItem(chatSeenKey(cwd), String(ts));
  } catch {
    // Storage blocked: the "new" count just never clears across restarts.
  }
}

const VIEW_NAMES: Record<Exclude<View["kind"], "task">, string> = {
  home: "Home",
  plan: "Plan",
  chat: "Chat",
  messages: "Chat",
  improvements: "Improvements",
  analytics: "Analytics",
  brainstorm: "Brainstorm",
  archive: "Archive",
};

function viewIcon(kind: View["kind"]) {
  switch (kind) {
    case "plan":
    case "brainstorm":
      return <ListTodo size={14} />;
    case "chat":
    case "messages":
      return <MessageSquare size={14} />;
    case "improvements":
      return <Sparkles size={14} />;
    case "analytics":
      return <ChartColumn size={14} />;
    default:
      return <LayoutList size={14} />;
  }
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
  // Set once the daemon has answered at least once: a later failure keeps
  // the last known task list on the rail instead of blanking the panel.
  const [lastSeenAt, setLastSeenAt] = useState<number | null>(null);
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
  const [reload, setReload] = useState(0);
  const [taskDraft, setTaskDraft] = useState("");
  const [baseBranchDraft, setBaseBranchDraft] = useState("");
  const [baseOpen, setBaseOpen] = useState(false);
  const [creatingBusy, setCreatingBusy] = useState(false);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const [focusComposer, setFocusComposer] = useState(0);
  // Tracked here, separately from `MessagesView`'s own copy, only so the
  // Messages segment can show a pending count before it is ever opened.
  const [messages, setMessages] = useState<Message[]>([]);
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  // The session the messages above belong to, and the repo's session list:
  // only a plain chat's replies count as "new" on the rail, not a brainstorm's.
  const [chatThreadId, setChatThreadId] = useState("");
  const [chatSessions, setChatSessions] = useState<KindedList | null>(null);
  const [chatSeen, setChatSeen] = useState<number | null>(() =>
    readChatSeen(cwd),
  );
  const [proposals, setProposals] = useState<Proposal[]>([]);

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
        setLastSeenAt(Date.now());
        setError("");
      })
      .catch((e) => {
        if (cancelled) return;
        const message = errorText(e);
        setError(message);
        setDaemonState(daemonDown(message) ?? "unavailable");
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
        if (event.event === "chat") {
          if (event.thread.repo === cwd) {
            setChatMessages(event.thread.messages);
            setChatThreadId(event.thread.id);
          }
          return;
        }
        if (event.event === "proposal") {
          if (event.proposal.repo === cwd)
            setProposals((old) => upsertProposal(old, event.proposal));
          return;
        }
        setLive((old) => applyOrchestratorEvent(old, event));
      }),
    [cwd],
  );

  useEffect(() => {
    let cancelled = false;
    // Each of these only feeds a count on the rail; a failure leaves it at
    // 0 until the next successful load, never blocking the panel.
    orchestratorClient
      .messageList(cwd)
      .then((loaded) => !cancelled && setMessages(loaded))
      .catch(() => {});
    orchestratorClient
      .chatGet(cwd)
      .then((thread) => {
        if (cancelled) return;
        setChatMessages(thread.messages);
        setChatThreadId(thread.id);
      })
      .catch(() => {});
    listSessions(cwd, "chat")
      .then((list) => !cancelled && setChatSessions(list))
      .catch(() => {});
    orchestratorClient
      .evolutionList(cwd)
      .then((loaded) => !cancelled && setProposals(loaded))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [cwd, reload]);

  const tasks = sortTasks(live.tasks.filter((t) => !t.archived));
  const archivedTasks = sortTasks(live.tasks.filter((t) => t.archived));
  const picked =
    view?.kind === "task" ? tasks.find((t) => t.id === view.id) : undefined;
  // Nothing picked (or the picked task was deleted): Home.
  const current: View =
    view && (view.kind !== "task" || picked) ? view : { kind: "home" };
  const selected = current.kind === "task" ? picked : undefined;
  const pendingMessages = messages.filter((m) => !m.delivered).length;
  const latestChat = chatMessages.reduce((max, m) => Math.max(max, m.ts), 0);
  const chatOpen = current.kind === "chat";

  // First load with nothing stored: everything so far counts as seen, so an
  // old history never shows as "new". Reading the chat marks it seen too.
  useEffect(() => {
    if (chatMessages.length === 0) return;
    if (chatSeen === null || (chatOpen && latestChat > chatSeen)) {
      setChatSeen(latestChat);
      writeChatSeen(cwd, latestChat);
    }
  }, [cwd, chatMessages.length, chatSeen, chatOpen, latestChat]);
  const chatIsChat =
    !chatThreadId || isOfKind(cwd, chatSessions, chatThreadId, "chat");
  const chatNew =
    chatSeen === null || chatOpen || !chatIsChat
      ? 0
      : unreadChatCount(chatMessages, chatSeen);

  useEffect(() => {
    if (focusComposer) composerRef.current?.focus();
  }, [focusComposer]);

  function open(next: View) {
    setView(next);
    setSwitcherOpen(false);
  }

  /** ⌘N: Home, with the task composer focused. */
  function newTask() {
    if (current.kind !== "home" && current.kind !== "plan")
      open({ kind: "home" });
    setSwitcherOpen(false);
    setFocusComposer((n) => n + 1);
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
      setScrollTo(target);
    };
    take();
    return subscribeReveal(take);
  }, [cwd, setView]);
  useEffect(() => {
    if (!scrollTo || selected?.id !== scrollTo.taskId) return;
    const root = rootRef.current;
    const element =
      (scrollTo.focus === "question" && root?.querySelector(".td-question")) ||
      (scrollTo.focus === "report" && root?.querySelector(".td-report")) ||
      Array.from(root?.querySelectorAll(".td-attempt") ?? []).pop();
    if (!element) return;
    element.scrollIntoView({ block: "center" });
    setScrollTo(null);
  }, [scrollTo, selected?.id, live.tasks]);

  /** A failed call: shown as an error, or - when the daemon itself is gone -
   * as the offline banner, keeping the last known task list on the rail. */
  function fail(e: unknown) {
    const message = errorText(e);
    setError(message);
    const down = daemonDown(message);
    if (down) setDaemonState(down);
  }

  async function act(action: () => Promise<Task>) {
    setBusy(true);
    try {
      const task = await action();
      setLive((old) => ({ ...old, tasks: upsertTask(old.tasks, task) }));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  function setOrchestratorRoute(routeId: string) {
    updateSettings({ orchestrator: routeId });
  }

  /** The one settings write: optimistic, then the daemon's copy replaces it. */
  function updateSettings(patch: Partial<Settings>): Promise<void> {
    if (!settings) return Promise.resolve();
    setSettings({ ...settings, ...patch });
    return orchestratorClient
      .settingsSet({ ...settings, ...patch })
      .then((saved) => setSettings(saved))
      .catch((e) => {
        setSettings(settings);
        fail(e);
        throw e;
      });
  }

  async function submitNewTask(start: boolean) {
    const text = taskDraft.trim();
    if (!text || creatingBusy) return;
    setCreatingBusy(true);
    setError("");
    try {
      const task = await createTask(cwd, text, baseBranchDraft, start);
      setLive((old) => ({ ...old, tasks: upsertTask(old.tasks, task) }));
      setTaskDraft("");
      setBaseBranchDraft("");
      setBaseOpen(false);
      if (start) open({ kind: "task", id: task.id });
    } catch (e) {
      fail(e);
    } finally {
      setCreatingBusy(false);
    }
  }

  /** The task view's composer talks to the orchestrator; the reply is read
   * in Chat. */
  function askOrchestrator() {
    const text = taskDraft.trim();
    if (!text || creatingBusy) return;
    setCreatingBusy(true);
    setError("");
    enterKind(cwd, "chat")
      .then(() => orchestratorClient.chatSend(cwd, text))
      .then(() => {
        setTaskDraft("");
        open({ kind: "chat" });
      })
      .catch((e) => fail(e))
      .finally(() => setCreatingBusy(false));
  }

  async function deleteSelected() {
    if (!selected) return;
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
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  function retry() {
    setDaemonState("loading");
    setReload((n) => n + 1);
  }

  if (daemonState === "loading" && lastSeenAt === null && !error)
    return <div className="loading">Connecting to the orchestrator…</div>;

  const offline = daemonState !== "ready";
  const refresh = live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join();
  const needYou = tasks.filter(needsOwner).length;
  const running = tasks.filter(
    (t) =>
      t.status === "running" ||
      t.status === "drafting" ||
      t.status === "queued",
  ).length;
  const improvementsCount = proposals.filter(
    (p) => p.status === "proposed" || p.status === "revert_suggested",
  ).length;

  const rail = (
    <OrchRail
      view={current}
      tasks={live.tasks}
      maxAttempts={settings?.maxAttempts}
      planCount={planDrafts(live.tasks).length}
      chatNew={chatNew}
      improvementsCount={improvementsCount}
      archivedCount={archivedTasks.length}
      offline={offline}
      onOpen={open}
    />
  );

  const chatKind =
    !offline && (current.kind === "chat" || current.kind === "messages")
      ? current.kind
      : null;
  const composerMode: "task" | "plan" | "ask" | null =
    chatKind || current.kind === "brainstorm"
      ? null
      : current.kind === "task"
        ? "ask"
        : current.kind === "plan"
          ? "plan"
          : "task";

  function offlineBody() {
    if (daemonState === "not-built")
      return (
        <>
          <div data-orchestrator-not-built>
            <Banner
              tone="warning"
              title="The orchestrator isn't built yet."
              body="Build it once from the repository root, then Retry."
              action={{ label: "Retry", onClick: retry }}
            />
          </div>
          <div className="orch-command">
            <span className="orch-command-prompt">$</span>
            <code>npm run build:orchd</code>
            <button
              type="button"
              className="ui-button ghost"
              onClick={() =>
                void navigator.clipboard?.writeText("npm run build:orchd")
              }
            >
              Copy
            </button>
          </div>
        </>
      );
    const seen =
      lastSeenAt === null
        ? ""
        : ` · last seen ${formatDuration(Date.now() - lastSeenAt).replace(/ \d+s$/, "")} ago`;
    return (
      <>
        <Banner
          tone="danger"
          title="Can't reach the orchestrator daemon."
          body={`${error || "It may still be starting."}${seen}`}
          action={{
            label: daemonState === "loading" ? "Retrying…" : "Retry",
            onClick: retry,
            disabled: daemonState === "loading",
          }}
        />
        <p className="orch-offline-note">
          Tasks keep their state; nothing is lost while the daemon is down.
          Retry reconnects; if it keeps failing, restart sushiAI.
        </p>
      </>
    );
  }

  function mainView() {
    if (offline)
      return <div className="orch-view-scroll offline">{offlineBody()}</div>;
    switch (current.kind) {
      case "task":
        return (
          selected && (
            <TaskDetail
              selected={selected}
              tasks={live.tasks}
              settings={settings}
              busy={busy}
              logLines={live.logLines[selected.id]}
              act={act}
              onOpen={(id) => open({ kind: "task", id })}
              onDelete={() => void deleteSelected()}
            />
          )
        );
      case "improvements":
        return (
          <ImprovementsView
            cwd={cwd}
            refresh={refresh}
            onOpenTask={(id) => open({ kind: "task", id })}
          />
        );
      case "analytics":
        return (
          <AnalyticsView
            cwd={cwd}
            refresh={refresh}
            onOpenTask={(id) => open({ kind: "task", id })}
          />
        );
      case "plan":
        return (
          <PlanView
            tasks={live.tasks}
            settings={settings}
            busy={busy}
            act={act}
            onSettings={updateSettings}
            onOpen={(id) => open({ kind: "task", id })}
            onBrainstorm={() => open({ kind: "brainstorm" })}
          />
        );
      case "brainstorm":
        return (
          <BrainstormView
            cwd={cwd}
            tasks={live.tasks}
            onCreated={(task) =>
              setLive((old) => ({
                ...old,
                tasks: upsertTask(old.tasks, task),
              }))
            }
            onOpenTask={(id) => open({ kind: "task", id })}
            onPlan={() => open({ kind: "plan" })}
          />
        );
      case "archive":
        return (
          <div className="orch-view-scroll orch-archive">
            <h2 className="orch-view-title">Archive</h2>
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
        );
      case "chat":
      case "messages":
        return null;
      default:
        return (
          <HomeView
            cwd={cwd}
            tasks={live.tasks}
            maxAttempts={settings?.maxAttempts}
            busy={busy}
            act={act}
            onOpen={(id) => open({ kind: "task", id })}
            onOpenAnalytics={() => open({ kind: "analytics" })}
            onSendNote={async (text) => {
              await enterKind(cwd, "chat");
              await orchestratorClient.chatSend(cwd, text);
            }}
            onTry={(text) => {
              setTaskDraft(text);
              setFocusComposer((n) => n + 1);
            }}
          />
        );
    }
  }

  const switcherLabel =
    current.kind === "task"
      ? (selected?.title ?? "")
      : VIEW_NAMES[current.kind];

  return (
    <div
      ref={rootRef}
      className="orchestrator-panel"
      onKeyDown={(event) => {
        if (event.key === "Escape" && switcherOpen) setSwitcherOpen(false);
        if (
          event.metaKey &&
          !event.shiftKey &&
          !event.altKey &&
          event.key.toLowerCase() === "n"
        ) {
          event.preventDefault();
          newTask();
        }
      }}
    >
      {rail}
      <div className="orch-main">
        <div className="orch-switcher">
          <button
            type="button"
            className="orch-switcher-toggle"
            aria-expanded={switcherOpen}
            aria-haspopup="true"
            onClick={() => setSwitcherOpen((v) => !v)}
          >
            {selected ? (
              <span className={`ui-dot ui-tone-${taskTone(selected)}`} />
            ) : (
              viewIcon(current.kind)
            )}
            <span className="orch-switcher-label">{switcherLabel}</span>
            <ChevronDown size={12} />
          </button>
          <span className="orch-switcher-tags">
            {needYou > 0 && <Tag tone="warning">{needYou} need you</Tag>}
            {running > 0 && <Tag tone="info">{running} running</Tag>}
          </span>
          {switcherOpen && <div className="orch-switcher-menu">{rail}</div>}
        </div>
        {error && !offline && (
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
        <ChatView
          cwd={cwd}
          onOpenTask={(id) => open({ kind: "task", id })}
          kind={chatKind}
          settings={settings}
          tasks={tasks}
          pendingMessages={pendingMessages}
          onRouteChange={setOrchestratorRoute}
          onShow={(kind) => open({ kind })}
        />
        {!chatKind && mainView()}
        {composerMode && (
          <div className="orch-composer-dock">
            <Composer
              inputRef={composerRef}
              value={taskDraft}
              onChange={setTaskDraft}
              onSubmit={() =>
                composerMode === "ask"
                  ? askOrchestrator()
                  : void submitNewTask(composerMode === "task")
              }
              placeholder={
                offline
                  ? daemonState === "not-built"
                    ? "Build the orchestrator to send tasks"
                    : "Reconnect to send tasks"
                  : composerMode === "ask"
                    ? "Ask the orchestrator to start, check or answer a task…"
                    : composerMode === "plan"
                      ? "Describe a task to add to the plan…"
                      : "Describe a task…"
              }
              ariaLabel={
                composerMode === "ask" ? "Message the orchestrator" : "New task"
              }
              sendLabel={
                composerMode === "ask" ? "Send message" : "Create task"
              }
              disabled={offline}
              sending={creatingBusy}
              route={
                composerMode === "ask" && settings ? (
                  <OrchestratorRouteChip
                    settings={settings}
                    onRouteChange={setOrchestratorRoute}
                  />
                ) : (
                  <TaskRouteLabel />
                )
              }
              extra={
                composerMode !== "ask" &&
                (baseOpen ? (
                  <input
                    className="orch-composer-base"
                    aria-label="Base branch (optional)"
                    placeholder="Defaults to the current branch"
                    value={baseBranchDraft}
                    disabled={offline}
                    autoFocus
                    onChange={(event) => setBaseBranchDraft(event.target.value)}
                    onBlur={() => !baseBranchDraft.trim() && setBaseOpen(false)}
                  />
                ) : (
                  <button
                    type="button"
                    className="orch-composer-base-toggle"
                    disabled={offline}
                    title="Base branch (optional)"
                    onClick={() => setBaseOpen(true)}
                  >
                    <GitBranch size={12} /> Base branch
                  </button>
                ))
              }
            />
          </div>
        )}
      </div>
    </div>
  );
}
