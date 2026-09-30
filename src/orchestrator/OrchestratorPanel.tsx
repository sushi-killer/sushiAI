import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ArchiveRestore,
  ChartColumn,
  ChevronDown,
  LayoutList,
  ListTodo,
  MessageSquare,
  Sparkles,
  X,
} from "lucide-react";
import "./orchestrator.css";
import type { OrchestratorView } from "../types";
import type { TaskTarget } from "./notices";
import type { OrchestratorHost, Preflight } from "./types";
import { pendingReveal, subscribeReveal } from "./reveal";
import {
  useOrchestratorClient,
  OrchestratorHostProvider,
  useOrchestratorHost,
} from "./hostContext";
import {
  LOCAL,
  orchestratorClientFor,
  type OrchestratorClient,
} from "./client";
import {
  hostOf,
  hostName as hostNameOf,
  needsSetup,
  repoSuggestions,
  runningCount,
  shownHost,
  type DaemonReach,
} from "./hosts";
import { HostSelect } from "./HostSelect";
import { PreflightStrip, RemoteSetup, RepoPrompt } from "./RemoteHostViews";
import { useOrchestratorHosts } from "./useHosts";
import { useWorkspaceRepos } from "./workspaceRepos";
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
import type {
  ChatMessage,
  ChatMode,
  Message,
  Proposal,
  Settings,
  Task,
} from "./types";
import { Banner, Tag } from "./ui";
import { OrchRail } from "./OrchRail";
import { Composer, OrchestratorRouteChip, TaskRouteLabel } from "./Composer";
import { TaskDetail } from "./TaskDetail";
import { HomeView } from "./HomeView";
import { ChatView } from "./ChatView";
import { ImprovementsView } from "./Improvements";
import { AnalyticsView } from "./Analytics";
import { PlanView } from "./PlanView";

type DaemonState = DaemonReach;

/** How often an open panel checks its daemon still answers, and how long
 * one check may take before the daemon counts as gone. */
const PING_EVERY_MS = 4000;
const PING_TIMEOUT_MS = 3000;

/** What the main pane beside the rail shows: see `OrchestratorView`. */
type View = OrchestratorView;

/** The task composer creates a task directly through `task.create`, not a
 * chat round-trip with the orchestrator agent. It prefers the `request`
 * form (the planner drafts title/goal/criteria/verify), but a disabled
 * planner shouldn't dead-end the control - falling back to the plain
 * title/goal form keeps it working either way. `start: false` keeps it as a
 * draft on the Plan. */
async function createTask(
  orchestratorClient: OrchestratorClient,
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
  archive: "Archive",
};

function viewIcon(kind: View["kind"]) {
  switch (kind) {
    case "plan":
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

function OrchestratorBody({
  cwd,
  header,
  remote,
  onDaemon,
  view: savedView,
  onViewChange,
}: {
  cwd: string;
  /** The host selector, pinned at the top of the rail. */
  header: ReactNode;
  /** Set on a remote host: its name (the composer's "on <host>") and what
   * it offers (Home's preflight strip). */
  remote?: { name: string; preflight?: Preflight | null };
  /** Reports each change of the daemon's reachability to the selector. */
  onDaemon(state: DaemonState): void;
  /** The view the panel had when it was last open; seeds its own state. */
  view?: OrchestratorView;
  /** Reports the view after each change so it can be saved on the panel. */
  onViewChange(view: OrchestratorView | undefined): void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const host = useOrchestratorHost();
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
  // Asks the chat for a new session with a mode selected (the Plan page's
  // Brainstorm button); `n` changes on every ask.
  const [chatStart, setChatStart] = useState<{ mode: ChatMode; n: number }>();
  const [chatSeen, setChatSeen] = useState<number | null>(() =>
    readChatSeen(cwd),
  );
  const [proposals, setProposals] = useState<Proposal[]>([]);

  const reportDaemon = useRef(onDaemon);
  reportDaemon.current = onDaemon;
  useEffect(() => reportDaemon.current(daemonState), [daemonState]);

  // The subscribe relay doesn't tell the renderer when the daemon goes away,
  // so an open panel pings it: one that stops answering flips the panel to
  // the unreachable state within a few seconds, and one that answers again -
  // or answers as a new process, restarted in between - reloads the task
  // list, whose events the old subscription missed.
  const daemonRef = useRef(daemonState);
  daemonRef.current = daemonState;
  useEffect(() => {
    let inFlight = false;
    let pid: number | null = null;
    let cancelled = false;
    const check = () => {
      if (inFlight || daemonRef.current === "not-built") return;
      inFlight = true;
      let timer = 0;
      const timeout = new Promise<never>((_, reject) => {
        timer = window.setTimeout(
          () => reject(new Error("orchd did not respond")),
          PING_TIMEOUT_MS,
        );
      });
      Promise.race([orchestratorClient.probe(), timeout])
        .then((answer) => {
          if (cancelled) return;
          const restarted = pid !== null && answer.pid !== pid;
          pid = answer.pid;
          if (daemonRef.current === "unavailable") {
            setDaemonState("loading");
            setReload((n) => n + 1);
          } else if (restarted) setReload((n) => n + 1);
        })
        .catch((e) => {
          if (cancelled || daemonRef.current !== "ready") return;
          const message = errorText(e);
          setError(message);
          setDaemonState(daemonDown(message) ?? "unavailable");
        })
        .finally(() => {
          window.clearTimeout(timer);
          inFlight = false;
        });
    };
    const interval = window.setInterval(check, PING_EVERY_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [orchestratorClient]);

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
  }, [cwd, reload, orchestratorClient]);

  useEffect(
    () =>
      // Task notices are raised by the main process's subscribe relay
      // (electron/orchestrator.cjs), not here: this panel only needs to
      // exist for a task's live state to update.
      window.bridge?.onOrchestrator((event) => {
        if (hostOf(event) !== host) return;
        if (event.event === "task" && event.task.repo !== cwd) return;
        if (event.event === "message") {
          if (event.message.repo === cwd)
            setMessages((old) => upsertMessage(old, event.message));
          return;
        }
        if (event.event === "chat") {
          if (event.thread.repo === cwd) setChatMessages(event.thread.messages);
          return;
        }
        if (event.event === "proposal") {
          if (event.proposal.repo === cwd)
            setProposals((old) => upsertProposal(old, event.proposal));
          return;
        }
        setLive((old) => applyOrchestratorEvent(old, event));
      }),
    [cwd, host],
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
      })
      .catch(() => {});
    orchestratorClient
      .evolutionList(cwd)
      .then((loaded) => !cancelled && setProposals(loaded))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [cwd, reload, orchestratorClient]);

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
  const chatNew =
    chatSeen === null || chatOpen ? 0 : unreadChatCount(chatMessages, chatSeen);

  useEffect(() => {
    if (focusComposer) composerRef.current?.focus();
  }, [focusComposer]);

  // First run: the composer is the only thing to do, so it takes the focus -
  // unless the owner is typing somewhere outside this panel.
  const firstRun =
    daemonState === "ready" &&
    current.kind === "home" &&
    live.tasks.every((t) => t.archived);
  useEffect(() => {
    if (!firstRun) return;
    const active = document.activeElement;
    if (
      !active ||
      active === document.body ||
      rootRef.current?.contains(active)
    )
      composerRef.current?.focus();
  }, [firstRun]);

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
      if (!target || applied.current === target || hostOf(target) !== host)
        return;
      applied.current = target;
      setView({ kind: "task", id: target.taskId });
      setScrollTo(target);
    };
    take();
    return subscribeReveal(take);
  }, [cwd, setView, host]);
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
      const task = await createTask(
        orchestratorClient,
        cwd,
        text,
        baseBranchDraft,
        start,
      );
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
    orchestratorClient
      .chatSend(cwd, text)
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

  const connecting = daemonState === "loading" && lastSeenAt === null && !error;
  const offline = daemonState !== "ready";
  const refresh = live.tasks.map((t) => `${t.id}:${t.updatedAt}`).join();
  const needYou = tasks.filter(needsOwner).length;
  const running = runningCount(tasks);
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
      offline={offline && !connecting}
      header={header}
      bare={connecting}
      onOpen={open}
    />
  );

  const chatKind =
    !offline && (current.kind === "chat" || current.kind === "messages")
      ? current.kind
      : null;
  // Figma has no composer on Improvements or Analytics.
  const composerMode: "task" | "plan" | "ask" | null =
    chatKind || current.kind === "improvements" || current.kind === "analytics"
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
        : ` · last seen ${formatDuration(Date.now() - lastSeenAt)} ago`;
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
    if (connecting)
      return <div className="loading">Connecting to the orchestrator…</div>;
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
            onBrainstorm={() => {
              setChatStart((old) => ({
                mode: "brainstorm",
                n: (old?.n ?? 0) + 1,
              }));
              open({ kind: "chat" });
            }}
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
              await orchestratorClient.chatSend(cwd, text);
            }}
            onTry={(text) => {
              setTaskDraft(text);
              setFocusComposer((n) => n + 1);
            }}
            remoteHost={remote?.name}
            hostStrip={
              remote?.preflight && (
                <PreflightStrip
                  hostName={remote.name}
                  preflight={remote.preflight}
                />
              )
            }
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
          start={chatStart}
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
              host={remote ? `on ${remote.name}` : undefined}
              route={
                composerMode === "ask" && settings ? (
                  <OrchestratorRouteChip
                    settings={settings}
                    onRouteChange={setOrchestratorRoute}
                  />
                ) : (
                  <TaskRouteLabel
                    disabled={offline}
                    onBaseBranch={() => setBaseOpen(true)}
                  />
                )
              }
              extra={
                composerMode !== "ask" &&
                baseOpen && (
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
                )
              }
            />
          </div>
        )}
      </div>
    </div>
  );
}

/** The states a host passed through while this panel watched it, when it
 * entered the last one, and its last failure: the setup card's step
 * details, its timer, and what to keep showing while the main process
 * retries on its own. `retry` forgets the failure (the owner's Try again). */
function useHostTrail(host: string, info: OrchestratorHost | undefined) {
  const state = info?.state;
  const detail = info?.detail ?? "";
  const fresh = (): HostTrail => ({
    host,
    states: state ? [state] : [],
    since: Date.now(),
    lastError: state === "error" ? detail : "",
  });
  const [trail, setTrail] = useState<HostTrail>(fresh);
  useEffect(() => {
    setTrail((old) => {
      if (old.host !== host)
        return {
          host,
          states: state ? [state] : [],
          since: Date.now(),
          lastError: state === "error" ? detail : "",
        };
      const lastError =
        state === "error"
          ? detail
          : state === "connecting" || state === "idle"
            ? old.lastError
            : "";
      if (
        !state ||
        (old.states.at(-1) === state && old.lastError === lastError)
      )
        return old;
      return {
        host,
        states:
          old.states.at(-1) === state ? old.states : [...old.states, state],
        since: old.states.at(-1) === state ? old.since : Date.now(),
        lastError,
      };
    });
  }, [host, state, detail]);
  const current = trail.host === host ? trail : fresh();
  const retry = () => setTrail((old) => ({ ...old, lastError: "" }));
  return { trail: current, retry };
}

type HostTrail = {
  host: string;
  states: OrchestratorHost["state"][];
  since: number;
  lastError: string;
};

/** The SSH address ("user@host") of each Connections profile, by host id. */
function useHostAddresses(hosts: OrchestratorHost[]): Record<string, string> {
  const [addresses, setAddresses] = useState<Record<string, string>>({});
  const ids = hosts.map((host) => host.id).join();
  useEffect(() => {
    let cancelled = false;
    window.bridge
      ?.connectionsList()
      .then((profiles) => {
        if (cancelled) return;
        setAddresses(
          Object.fromEntries(
            profiles.map((profile) => [`ssh:${profile.id}`, profile.host]),
          ),
        );
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [ids]);
  return addresses;
}

/** The Orchestrator panel: the host selector (Local + each SSH profile) at
 * the top of the rail, above the task list, new-task form, chat and
 * settings of the chosen host's daemon. A remote host that is still being
 * set up shows its setup steps instead. In a workspace opened on a host the
 * panel starts on that host and its folder; any other host asks for a repo
 * path on it. */
export function OrchestratorPanel({
  cwd,
  endpoint,
  view,
  host: savedHost,
  repo: savedRepo,
  onViewChange,
  onHostChange,
  onAddHost,
}: {
  cwd: string;
  /** The workspace's own host: "ssh:<id>" or nothing for this machine. */
  endpoint?: string;
  /** The view the panel had when it was last open; seeds its own state. */
  view?: OrchestratorView;
  /** The host and repo the owner picked in this panel; seeds its own state. */
  host?: string;
  repo?: string;
  /** Reports the view after each change so it can be saved on the panel. */
  onViewChange(view: OrchestratorView | undefined): void;
  onHostChange(choice: { host: string; repo?: string }): void;
  /** Opens Settings -> Connections, where SSH hosts are added. */
  onAddHost?: () => void;
}) {
  const workspaceHost = endpoint?.startsWith("ssh:") ? endpoint : LOCAL;
  const { hosts, refresh } = useOrchestratorHosts();
  const workspaces = useWorkspaceRepos();
  const host = savedHost ?? workspaceHost;
  const remote = host !== LOCAL;
  // The host's tasks, kept per host: a failed reload (the daemon went away)
  // keeps the last list, and with it the repo picked from it.
  const [hostTasks, setHostTasks] = useState<{ host: string; list: Task[] }>({
    host,
    list: [],
  });
  const tasks = hostTasks.host === host ? hostTasks.list : [];
  // Bumped by Try again: asks the host's daemon once more, which re-runs
  // the main process's setup when it failed.
  const [attempt, setAttempt] = useState(0);
  const [daemon, setDaemon] = useState<DaemonState | undefined>();
  // Hosts whose daemon answered this panel at least once: losing one later
  // keeps its tasks on screen behind the offline banner, not the setup card.
  const [answered, setAnswered] = useState<ReadonlySet<string>>(new Set());
  const [running, setRunning] = useState<Record<string, number>>({});
  const rawInfo = hosts.find((item) => item.id === host);
  const { trail, retry } = useHostTrail(host, rawInfo);
  const info = rawInfo && shownHost(rawInfo, trail.lastError);
  const ready = info?.state === "ready";
  const addresses = useHostAddresses(hosts);

  // The host's own tasks give its repo suggestions (and a default repo);
  // asking for them is also what starts a remote host's setup.
  useEffect(() => {
    if (host === LOCAL) return;
    let cancelled = false;
    orchestratorClientFor(host)
      .taskList()
      .then((list) => !cancelled && setHostTasks({ host, list }))
      .catch(() => {})
      .finally(() => !cancelled && refresh());
    return () => {
      cancelled = true;
    };
  }, [host, ready, attempt, refresh]);

  const suggestions = repoSuggestions(host, workspaces, tasks);
  const repo =
    savedRepo ?? (host === workspaceHost ? cwd : (suggestions[0] ?? ""));
  const choose = useCallback(
    (next: { host: string; repo?: string }) => {
      onViewChange(undefined);
      setDaemon(undefined);
      onHostChange(next);
    },
    [onViewChange, onHostChange],
  );

  // A notice for a task of another host lands here first: the panel switches
  // to that host, then the body below shows the task.
  const applied = useRef<unknown>(null);
  useEffect(() => {
    const take = () => {
      const target = pendingReveal(cwd);
      if (!target || applied.current === target) return;
      applied.current = target;
      const targetHost = hostOf(target);
      const local = targetHost === workspaceHost;
      if (targetHost !== host || (!local && target.repo !== repo))
        choose({ host: targetHost, repo: local ? undefined : target.repo });
    };
    take();
    return subscribeReveal(take);
  }, [cwd, host, repo, workspaceHost, choose]);

  const onDaemon = useCallback(
    (state: DaemonState) => {
      setDaemon(state);
      if (state === "ready")
        setAnswered((old) => (old.has(host) ? old : new Set(old).add(host)));
    },
    [host],
  );

  /** The running count of every host that can answer, for the menu. */
  function loadRunning() {
    for (const item of hosts) {
      if (item.id !== LOCAL && item.state !== "ready") continue;
      orchestratorClientFor(item.id)
        .taskList()
        .then((list) =>
          setRunning((old) => ({ ...old, [item.id]: runningCount(list) })),
        )
        .catch(() => {});
    }
  }

  const name = info?.name ?? hostNameOf(hosts, host);
  const hostSelect = (
    <HostSelect
      hosts={hosts.map((item) => (item.id === host && info ? info : item))}
      current={host}
      daemon={daemon}
      running={running}
      repo={repo}
      suggestions={suggestions}
      onChoose={(next) => choose({ host: next })}
      onRepo={(next) => onHostChange({ host, repo: next })}
      onRecheck={() => {
        void window.bridge
          ?.orchestratorPreflight(host)
          .then(refresh)
          .catch(refresh);
      }}
      onAddHost={onAddHost}
      onOpen={loadRunning}
    />
  );

  const settingUp =
    remote && info !== undefined && needsSetup(info, answered.has(host));
  if (settingUp || (remote && !repo)) {
    const content =
      settingUp && info ? (
        <RemoteSetup
          host={info}
          seen={trail}
          address={addresses[host]}
          onRetry={() => {
            retry();
            // Clears the main process's failure pause before asking again.
            void window.bridge?.orchestratorPreflight(host).catch(() => {});
            setAttempt((n) => n + 1);
          }}
          onCancel={() =>
            choose({ host: workspaceHost === host ? LOCAL : workspaceHost })
          }
        />
      ) : (
        <RepoPrompt
          hostName={name}
          suggestions={suggestions}
          onRepo={(next) => onHostChange({ host, repo: next })}
        />
      );
    return (
      <div className="orchestrator-host">
        <div className="orchestrator-panel">
          <OrchRail
            view={{ kind: "home" }}
            tasks={[]}
            planCount={0}
            chatNew={0}
            improvementsCount={0}
            archivedCount={0}
            header={hostSelect}
            bare
            onOpen={() => {}}
          />
          <div className="orch-main">
            <div className="orch-switcher">{hostSelect}</div>
            {content}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="orchestrator-host">
      <OrchestratorHostProvider value={host}>
        <OrchestratorBody
          key={`${host}\n${repo}`}
          cwd={repo}
          header={hostSelect}
          remote={remote ? { name, preflight: info?.preflight } : undefined}
          onDaemon={onDaemon}
          view={savedHost === host || !savedHost ? view : undefined}
          onViewChange={onViewChange}
        />
      </OrchestratorHostProvider>
    </div>
  );
}
