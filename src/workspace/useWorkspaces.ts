import { useCallback, useEffect, useRef, useState } from "react";
import { contains, leaf, remove, resize, split, uid } from "../layout.ts";
import { initialWorkspace } from "../workspaceState.ts";
import { LOCAL_ENDPOINT } from "../daemonSessions.ts";
import type { ClosedProject, Routine, Saved } from "../workspaceState.ts";
import { applyChatEvent, startUserTurn } from "../chat-threads.ts";
import { disposeTerminal } from "../TerminalPanel.tsx";
import { errorText, isGone } from "../lib/errors.ts";
import { agentTitle } from "../app/agent-title.ts";
import type { ProjectGit } from "../app/useProjectGit.ts";
import {
  closeBeforeWorktreeRemoval,
  type WorktreeCleanupRequest,
} from "../app/worktreeCleanup.ts";
import {
  closedProjectId,
  forgetProject as removeClosedProject,
  refreshProject,
  rememberProject,
} from "../app/projects.ts";
import {
  appendPanel,
  findPanelOwner,
  fixSelection,
  groupPanelIds,
  closeRequest,
  movePanel as moveInLayout,
  openCompanion as openCompanionIn,
  patchCompanion as patchCompanionIn,
  patchPanel,
  type PanelUpdate,
  removeClosedSessions,
  removePanel,
  renameRequest,
  reopenRequest,
  tidyGroupLayout,
  tidyWorkspace,
} from "./workspace-actions.ts";
import type { GroupCanvasContext } from "./workspace-actions.ts";
import { launchesInWorktree } from "./worktree.ts";
import {
  launchDaemonSession,
  launchTarget,
  newWorkspaceId,
  placeLaunchedPanel,
} from "./session-launch.ts";
import { hostOf } from "../projectWorktrees.ts";
import type {
  ModelProfile,
  Panel,
  PanelKind,
  SessionLaunchRequest,
  Workspace,
} from "../types";

export type WorkspaceController = ReturnType<typeof useWorkspaces>;

/** Ends a panel's live session gracefully; a session that is already gone is
 * the outcome asked for. */
async function closeSession(
  workspace: Workspace,
  panel: Panel,
  endpoint: string,
) {
  const request = closeRequest(workspace, panel, endpoint);
  if (!request) return;
  await window
    .bridge!.sessionClose(request.host, request.id, request.graceful)
    .catch((error) => {
      if (!isGone(error)) throw error;
    });
}

/** Owns everything about the panel and workspace lifecycle. The workspace list
 * itself lives in App because the daemon controller writes into it too; this hook
 * and that one are its only mutators. */
export function useWorkspaces({
  workspaces,
  setWorkspaces,
  saved,
  notify,
  showWorkspace,
  confirmClose,
}: {
  workspaces: Workspace[];
  setWorkspaces: React.Dispatch<React.SetStateAction<Workspace[]>>;
  saved: Saved | null;
  notify(text: string): void;
  showWorkspace(): void;
  confirmClose(target: { workspace: Workspace; panel: Panel }): void;
}) {
  const [activeId, setActiveId] = useState(saved?.activeId || "");
  const [selected, setSelected] = useState(saved?.selected || "");
  const [zoomed, setZoomed] = useState<string | null>(saved?.zoomed || null);
  const [dragId, setDragId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const launches = useRef(0);
  const reopening = useRef(new Map<string, Promise<void>>());
  const [closedProjects, setClosedProjects] = useState<ClosedProject[]>(
    saved?.closedProjects || [],
  );
  const active = workspaces.find((w) => w.id === activeId) || workspaces[0];
  const projectGitRef = useRef<Record<string, ProjectGit>>({});
  const setProjectGit = useCallback((next: Record<string, ProjectGit>) => {
    projectGitRef.current = next;
  }, []);
  const activeRef = useRef(active);
  const workspacesRef = useRef(workspaces);
  activeRef.current = active;
  workspacesRef.current = workspaces;
  const zoomedRef = useRef(zoomed);
  const dragIdRef = useRef(dragId);
  zoomedRef.current = zoomed;
  dragIdRef.current = dragId;
  // The active merge group supplies member panes to selection and canvas operations.
  const groupRef = useRef<GroupCanvasContext | undefined>(undefined);

  useEffect(() => {
    const next = fixSelection(
      active,
      selected,
      zoomed,
      groupRef.current ? groupPanelIds(groupRef.current.group) : undefined,
    );
    if (next.selected !== selected) setSelected(next.selected);
    if (next.zoomed !== zoomed) setZoomed(next.zoomed);
  }, [active.id, active.layout, active.panels, selected, zoomed]);
  const updateWorkspace = useCallback(
    (workspaceId: string, update: (w: Workspace) => Workspace) =>
      setWorkspaces((items) =>
        items.map((w) => (w.id === workspaceId ? update(w) : w)),
      ),
    [],
  );
  const updatePanel = useCallback(
    (panelId: string, patch: PanelUpdate) =>
      setWorkspaces((items) => patchPanel(items, panelId, patch)),
    [],
  );
  const openCompanion = useCallback(
    (
      panelId: string,
      target: { extensionId: string; surfaceId: string },
      args: Record<string, string>,
    ) =>
      setWorkspaces((items) => openCompanionIn(items, panelId, target, args)),
    [],
  );
  const patchCompanion = useCallback(
    (panelId: string, patch: Parameters<typeof patchCompanionIn>[2]) =>
      setWorkspaces((items) => patchCompanionIn(items, panelId, patch)),
    [],
  );
  const focusPanel = useCallback((panelId: string) => {
    setSelected(panelId);
  }, []);
  const startPanelDrag = useCallback((panelId: string) => {
    dragIdRef.current = panelId;
    setDragId(panelId);
  }, []);
  const endPanelDrag = useCallback(() => {
    dragIdRef.current = null;
    setDragId(null);
  }, []);
  const zoomPanel = useCallback((panelId: string) => {
    setZoomed((current) => (current === panelId ? null : panelId));
  }, []);

  const navigatePanel = useCallback(
    (panelId: string, url: string) =>
      updatePanel(panelId, { url, previewFile: undefined }),
    [updatePanel],
  );
  const setPanelAgent = useCallback(
    (panelId: string, agent: string) => updatePanel(panelId, { agent }),
    [updatePanel],
  );
  const cancelPanelChat = useCallback(
    (panelId: string) =>
      window.bridge
        ?.cancelChat(panelId)
        .catch((error) => notify(errorText(error))),
    [notify],
  );
  useEffect(() => {
    if (!window.bridge) return;
    return window.bridge.onChat((event) =>
      setWorkspaces((items) =>
        items.map((w) => ({
          ...w,
          panels: w.panels.map((p) =>
            p.id === event.panelId ? applyChatEvent(p, event) : p,
          ),
        })),
      ),
    );
  }, [notify]);
  const renamePanel = useCallback(
    (panelId: string, title: string) => {
      const owner = findPanelOwner(workspacesRef.current, panelId);
      const panel = owner?.panels.find((item) => item.id === panelId);
      if (!owner || !panel) return;
      const request = renameRequest(owner, panel, title, LOCAL_ENDPOINT);
      if (request) {
        window.bridge
          ?.sessionUpdate(request.host, request.patch)
          .then(() => updatePanel(panelId, { title }))
          .catch((error) => notify(errorText(error)));
      } else {
        updatePanel(panelId, { title });
      }
    },
    [notify, updatePanel],
  );
  /** "Hide only": drop the pane from the layout but leave its process running.
   * The session and the transcript are untouched. */
  function hidePanel(workspaceId: string, panelId: string) {
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      layout: remove(w.layout, panelId),
    }));
    disposeTerminal(panelId);
    setZoomed(null);
  }
  /** Renames a workspace. The name is the desktop's; daemons learn it with the
   * catalog sync, which is not part of this slice. */
  async function renameWorkspace(workspaceId: string, name: string) {
    if (!workspacesRef.current.some((w) => w.id === workspaceId)) return;
    updateWorkspace(workspaceId, (w) => ({ ...w, name }));
  }
  async function launchSession(
    request: SessionLaunchRequest,
    template: Panel,
    restore?: { workspaceId: string; panelId: string },
    afterLaunch?: (host: string, sessionId: string) => Promise<void>,
  ): Promise<boolean> {
    launches.current += 1;
    setAdding(true);
    try {
      if (!window.bridge) throw new Error("Open the desktop app first.");
      const {
        host,
        sessionId,
        cwd,
        panel: bound,
      } = await launchDaemonSession(window.bridge, request, template);
      const panel: Panel = { ...bound, started: request.kind === "agent" };
      const targetId =
        launchTarget(workspacesRef.current, request, panel.id, restore) ||
        request.workspaceId ||
        uid();
      setWorkspaces((items) =>
        placeLaunchedPanel(items, { targetId, request, panel, cwd, restore }),
      );
      switchWorkspace(targetId);
      setSelected(panel.id);
      if (restore) {
        disposeTerminal(restore.panelId);
        if (zoomedRef.current === restore.panelId) setZoomed(panel.id);
      }
      await afterLaunch?.(host, sessionId);
      return true;
    } catch (error) {
      notify(errorText(error));
      return false;
    } finally {
      launches.current -= 1;
      setAdding(launches.current > 0);
    }
  }
  async function createWorkspace(
    name: string,
    cwd: string,
    starter: string,
    endpoint: string = activeRef.current.connection || LOCAL_ENDPOINT,
    operationId?: string,
    env?: Record<string, string>,
  ): Promise<boolean> {
    const kind = starter === "shell" ? "terminal" : "agent";
    const operation = operationId || uid();
    return launchSession(
      {
        operationId: operation,
        workspaceId: newWorkspaceId(operation),
        endpoint,
        cwd,
        label: name,
        kind,
        agent: kind === "agent" ? starter : undefined,
        env,
      },
      {
        id: "",
        kind,
        title: kind === "terminal" ? "zsh" : agentTitle(starter),
        agent: kind === "agent" ? starter : undefined,
      },
      undefined,
    );
  }
  /** Places a panel App already built (an extension surface) into a
   * workspace - the merged row's chosen host (D2) when given, else the
   * active one, exactly like addPanel. */
  function insertPanel(panel: Panel, targetWorkspaceId?: string) {
    const current =
      (targetWorkspaceId &&
        workspacesRef.current.find((w) => w.id === targetWorkspaceId)) ||
      activeRef.current;
    updateWorkspace(current.id, (w) => appendPanel(w, panel));
    setSelected(panel.id);
    if (current.id !== activeRef.current.id) switchWorkspace(current.id);
    else {
      showWorkspace();
      setZoomed(null);
    }
  }
  /** The soft variant used by the Chat list: follow the project, but leave the
   * current view and zoom alone. */
  function selectWorkspace(id: string) {
    setActiveId(id);
  }
  function switchWorkspace(id: string) {
    setActiveId(id);
    showWorkspace();
    setZoomed(null);
  }
  async function addPanel(
    kind: PanelKind,
    agent = "claude",
    filesTarget?: Panel["filesTarget"],
    modelProfile?: ModelProfile,
    accountId?: string,
    targetWorkspaceId?: string,
    worktree?: { branch: string; base?: string },
    operationId?: string,
    env?: Record<string, string>,
    prompt?: string,
  ): Promise<boolean> {
    const modelProfileId = modelProfile?.id;
    const accounts =
      kind !== "agent"
        ? {}
        : agent === "claude"
          ? { claudeAccountId: accountId }
          : agent === "codex"
            ? { codexAccountId: accountId }
            : {};
    // A merged-row session host choice (D2): defaults to the active
    // workspace, same as before targetWorkspaceId existed.
    const current =
      (targetWorkspaceId &&
        workspacesRef.current.find((w) => w.id === targetWorkspaceId)) ||
      activeRef.current;
    try {
      // Every agent and terminal panel runs in a daemon: the local one for a
      // workspace without a connection, the host's own for an ssh one.
      if (prompt && !launchesInWorktree(kind))
        throw new Error("Starting with a prompt needs an agent or terminal.");
      if (launchesInWorktree(kind)) {
        const endpoint = current.connection || LOCAL_ENDPOINT;
        const operation = operationId || uid();
        return await launchSession(
          {
            operationId: operation,
            endpoint,
            cwd: current.cwd,
            // The session title: the panel's own, or the new worktree workspace's.
            label: worktree
              ? `${current.name} · ${worktree.branch}`
              : kind === "agent"
                ? modelProfile?.label || agentTitle(agent)
                : "zsh",
            kind: kind === "agent" ? "agent" : "terminal",
            agent: kind === "agent" ? agent : undefined,
            modelProfileId,
            ...accounts,
            env,
            // A worktree launch is a workspace of its own: its id is the group.
            workspaceId: worktree ? newWorkspaceId(operation) : current.id,
            worktree,
            ...(prompt ? { prompt } : {}),
          },
          {
            id: "",
            kind,
            title:
              kind === "agent"
                ? modelProfile?.label || agentTitle(agent)
                : "zsh",
            agent: kind === "agent" ? agent : undefined,
            modelProfileId,
            ...accounts,
          },
          undefined,
        );
      } else {
        const panel: Panel = {
          id: uid(),
          kind,
          title:
            kind === "agent"
              ? modelProfile?.label || agentTitle(agent)
              : kind === "terminal"
                ? "zsh"
                : kind === "browser"
                  ? "Browser"
                  : kind === "files"
                    ? "Files & Git"
                    : "Thread",
          agent: kind === "agent" || kind === "chat" ? agent : undefined,
          started: kind === "agent",
          messages: kind === "chat" ? [] : undefined,
          filesTarget: kind === "files" ? filesTarget : undefined,
          modelProfileId: kind === "agent" ? modelProfileId : undefined,
          ...accounts,
        };
        updateWorkspace(current.id, (w) => appendPanel(w, panel));
        setSelected(panel.id);
      }
      // D2: a chosen host other than the active workspace becomes active too,
      // so the new pane (just selected above) is actually visible.
      if (current.id !== activeRef.current.id) switchWorkspace(current.id);
      else {
        showWorkspace();
        setZoomed(null);
      }
      return true;
    } catch (error) {
      notify(errorText(error));
      return false;
    }
  }
  function reopenPanel(panelId: string, operationId?: string): Promise<void> {
    const queued = reopening.current.get(panelId);
    if (queued) return queued;
    const owner = findPanelOwner(workspacesRef.current, panelId);
    const ended = owner?.panels.find((item) => item.id === panelId);
    if (
      !owner ||
      !ended ||
      (ended.kind !== "agent" && ended.kind !== "terminal") ||
      (!ended.ended && ended.sessionId)
    )
      return Promise.resolve();
    const request: SessionLaunchRequest = reopenRequest(
      owner,
      ended,
      operationId || uid(),
      LOCAL_ENDPOINT,
    );
    const run = Promise.resolve()
      .then(() =>
        launchSession(request, ended, { workspaceId: owner.id, panelId }),
      )
      .then(() => {});
    reopening.current.set(panelId, run);
    void run.finally(() => {
      if (reopening.current.get(panelId) === run)
        reopening.current.delete(panelId);
    });
    return run;
  }
  /** A panel that never ran starts like a Reopen: a new daemon session in its slot. */
  const startPanel = (panelId: string) => void reopenPanel(panelId);
  const endSessions = useCallback(
    async (
      items: { workspace: Workspace; panel: Panel }[],
      cleanup?: WorktreeCleanupRequest,
    ) => {
      const closed = new Set<string>(),
        errors: string[] = [];
      for (const { workspace, panel } of items) {
        const outcome = await closeBeforeWorktreeRemoval(
          async () => {
            if (panel.sessionId)
              await closeSession(workspace, panel, LOCAL_ENDPOINT);
            if (panel.busy) await window.bridge?.cancelChat(panel.id);
            disposeTerminal(panel.id);
          },
          cleanup?.panel.id === panel.id
            ? async () =>
                window.bridge?.worktreeRemove(
                  hostOf(cleanup.workspace.connection),
                  cleanup.checkout,
                  cleanup.checkout,
                  { branch: cleanup.branch },
                )
            : undefined,
        );
        if (!outcome.closed) {
          errors.push(panel.title + ": " + errorText(outcome.closeError));
          continue;
        }
        closed.add(panel.id);
        if ("cleanupError" in outcome) {
          errors.push(`Worktree kept: ${errorText(outcome.cleanupError)}`);
        }
      }
      setWorkspaces((list) => {
        const remaining = removeClosedSessions(list, closed);
        return remaining.length ? remaining : [initialWorkspace()];
      });
      if (zoomedRef.current && closed.has(zoomedRef.current)) setZoomed(null);
      if (errors.length) notify(errors.join("; "));
    },
    [notify, setWorkspaces],
  );
  const closePanel = useCallback(
    (panelId: string) => {
      const owner = findPanelOwner(workspacesRef.current, panelId);
      const panel = owner?.panels.find((item) => item.id === panelId);
      if (!owner || !panel) return;
      if (owner.localWorktree && launchesInWorktree(panel.kind)) {
        confirmClose({ workspace: owner, panel });
        return;
      }
      // Closing a live session ends it, so the owner always confirms first.
      if (panel.sessionId && !panel.ended) {
        confirmClose({ workspace: owner, panel });
        return;
      }
      disposeTerminal(panel.id);
      // An ended pane has no session left to end: drop it outright.
      if (panel.ended) {
        setWorkspaces((list) => {
          const remaining = removeClosedSessions(list, new Set([panel.id]));
          return remaining.length ? remaining : [initialWorkspace()];
        });
        if (zoomedRef.current === panel.id) setZoomed(null);
        return;
      }
      // Closing a session panel hides it locally; it never kills the session.
      if (panel.busy)
        window.bridge
          ?.cancelChat(panel.id)
          .catch((error) => notify(errorText(error)));
      // A pane is a view, a thread is a conversation: closing the view leaves the
      // Code layout only. Threads are deleted from the Chat tab.
      updateWorkspace(owner.id, (w) => removePanel(w, panel));
      if (zoomedRef.current === panel.id) setZoomed(null);
    },
    [confirmClose, endSessions, notify, setWorkspaces, updateWorkspace],
  );
  function showPanel(panel: Panel) {
    showWorkspace();
    setSelected(panel.id);
    setZoomed(panel.id);
    updateWorkspace(active.id, (w) => ({
      ...w,
      layout: w.layout || leaf(panel.id),
    }));
  }
  const drop = useCallback(
    (target: string, edge: string) => {
      const source = dragIdRef.current;
      const group = groupRef.current;
      // Drag-swap across members (C2): the combined canvas draws the merge
      // group's own layout, not the active workspace's, so a drop inside it
      // moves panes within that layout instead - source and target can each
      // belong to a different member.
      if (group) {
        if (
          source &&
          source !== target &&
          group.layout &&
          contains(group.layout, source) &&
          contains(group.layout, target)
        )
          group.setLayout(moveInLayout(group.layout, source, target, edge));
        endPanelDrag();
        return;
      }
      const current = activeRef.current;
      if (
        !source ||
        source === target ||
        !current.layout ||
        !contains(current.layout, source) ||
        !contains(current.layout, target)
      ) {
        endPanelDrag();
        return;
      }
      updateWorkspace(current.id, (w) => ({
        ...w,
        layout: moveInLayout(w.layout, source, target, edge),
      }));
      endPanelDrag();
    },
    [endPanelDrag, updateWorkspace],
  );
  const sendChat = useCallback(
    async (panel: Panel, text: string, attachments: string[] = []) => {
      if (!window.bridge)
        return notify("Open the desktop app to chat with your agents.");
      const owner =
        workspacesRef.current.find((w) =>
          w.panels.some((p) => p.id === panel.id),
        ) || activeRef.current;
      const { messages, update } = startUserTurn(panel, text, attachments);
      updatePanel(panel.id, update);
      try {
        await window.bridge.chat({
          panelId: panel.id,
          cwd: owner.cwd,
          endpoint: owner.connection,
          agent: panel.agent || "claude",
          messages,
          model: panel.model || undefined,
          effort: panel.effort || undefined,
          permission: panel.permission || undefined,
        });
      } catch (error) {
        updatePanel(panel.id, { busy: false, error: errorText(error) });
      }
    },
    [notify, updatePanel],
  );
  // Chat threads live in the project's panel list but stay out of the Code layout.
  function newThread(workspaceId: string): Panel {
    const panel: Panel = {
      id: uid(),
      kind: "chat",
      title: "New thread",
      agent: "claude",
      permission: "acceptEdits",
      messages: [],
      updatedAt: Date.now(),
    };
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      panels: [...w.panels, panel],
    }));
    return panel;
  }
  function deleteThread(workspaceId: string, panel: Panel) {
    if (panel.busy)
      window.bridge
        ?.cancelChat(panel.id)
        .catch((error) => notify(errorText(error)));
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      layout: remove(w.layout, panel.id),
      panels: w.panels.filter((p) => p.id !== panel.id),
    }));
    if (zoomed === panel.id) setZoomed(null);
  }
  async function runRoutine(routine: Routine) {
    if (!window.bridge) return notify("Run routines in the desktop app.");
    const bridge = window.bridge;
    // The shell holds the typed command until the pane attaches.
    await launchSession(
      {
        operationId: uid(),
        endpoint: active.connection || LOCAL_ENDPOINT,
        cwd: active.cwd,
        label: routine.name,
        kind: "terminal",
        workspaceId: active.id,
      },
      { id: "", kind: "terminal", title: routine.name },
      undefined,
      (host, sessionId) =>
        bridge.sessionInput(host, sessionId, routine.command + "\r"),
    );
    showWorkspace();
  }

  const closing = useRef(new Set<string>());
  async function endWorkspace(workspace: Workspace) {
    // A second click while the first is closing it does nothing.
    if (closing.current.has(workspace.id)) return;
    closing.current.add(workspace.id);
    try {
      for (const panel of workspace.panels)
        if (panel.sessionId)
          await closeSession(workspace, panel, LOCAL_ENDPOINT);
      for (const panel of workspace.panels) {
        disposeTerminal(panel.id);
        if (panel.busy) await window.bridge?.cancelChat(panel.id);
      }
      if (workspace.cwd) rememberClosed(workspace);
      setWorkspaces((list) => {
        const rest = list.filter((w) => w.id !== workspace.id);
        return rest.length ? rest : [initialWorkspace()];
      });
      setZoomed(null);
    } catch (error) {
      notify(errorText(error));
    } finally {
      closing.current.delete(workspace.id);
    }
  }
  /** Keeps the just-closed workspace on the Dashboard (B1): a fresh git
   * identity read the same way `useProjectGit` reads one, since the
   * workspace is about to disappear and can no longer be looked up by id. */
  function rememberClosed(workspace: Workspace) {
    const endpoint = workspace.connection;
    const entry: ClosedProject = {
      id: closedProjectId(endpoint, workspace.cwd),
      name: workspace.name,
      cwd: workspace.cwd,
      endpoint,
      backed: Boolean(endpoint),
      closedAt: Date.now(),
      git: {
        projectId: "",
        remote: "",
        commonDir: "",
        checkout: "",
        linkedWorktree: false,
        subdir: "",
        branch: "",
      },
    };
    setClosedProjects((list) => rememberProject(list, entry));
    window.bridge
      ?.projectIdentify(workspace.connection, workspace.cwd)
      .then((result) =>
        setClosedProjects((list) =>
          refreshProject(list, {
            ...entry,
            git: {
              projectId: result?.projectId || "",
              remote: result?.remote || "",
              commonDir: result?.commonDir || "",
              checkout: result?.checkout || "",
              linkedWorktree: result?.linkedWorktree || false,
              subdir: result?.subdir || "",
              branch: result?.branch || "",
            },
          }),
        ),
      )
      .catch(() => {});
  }
  /** Reopens a remembered project through the path a fresh workspace already
   * uses - the daemon on its original host when it had one, else local - and
   * forgets it only once that succeeds, so a host that cannot be reached
   * (surfaced via `notify` inside `createWorkspace`) leaves the entry in
   * place to retry. */
  async function reopenProject(project: ClosedProject) {
    const ok = await createWorkspace(
      project.name,
      project.cwd,
      "shell",
      project.endpoint || LOCAL_ENDPOINT,
    );
    if (ok) forgetProject(project.id);
  }
  function forgetProject(id: string) {
    setClosedProjects((list) => removeClosedProject(list, id));
  }
  const openHTML = useCallback(
    (root: string, file: string, endpoint?: string) => {
      const current = activeRef.current;
      const panel: Panel = {
        id: uid(),
        kind: "browser",
        title: file.split("/").at(-1) || "Preview",
        previewFile: { root, path: file, endpoint },
      };
      updateWorkspace(current.id, (w) => ({
        ...w,
        panels: [...w.panels, panel],
        layout: w.layout
          ? split(w.layout, leaf(panel.id), "row", 0.55)
          : leaf(panel.id),
      }));
      setSelected(panel.id);
      setZoomed(panel.id);
    },
    [updateWorkspace],
  );

  return {
    workspaces,
    active,
    setProjectGit,
    activeId,
    selected,
    zoomed,
    dragId,
    adding,
    groupRef,
    setSelected,
    setZoomed,
    setActiveId,
    updateWorkspace,
    updatePanel,
    openCompanion,
    patchCompanion,
    focusPanel,
    startPanelDrag,
    endPanelDrag,
    zoomPanel,
    startPanel,
    reopenPanel,
    navigatePanel,
    setPanelAgent,
    cancelPanelChat,
    renamePanel,
    switchWorkspace,
    selectWorkspace,
    insertPanel,
    hidePanel,
    renameWorkspace,
    createWorkspace,
    addPanel,
    closePanel,
    showPanel,
    drop,
    sendChat,
    newThread,
    deleteThread,
    runRoutine,
    endSessions,
    endWorkspace,
    closedProjects,
    reopenProject,
    forgetProject,
    openHTML,
    tidy: () => {
      const group = groupRef.current;
      if (group) group.setLayout(tidyGroupLayout(group.group));
      else updateWorkspace(active.id, tidyWorkspace);
    },
    resizeSplit: (id: string, ratio: number) => {
      const group = groupRef.current;
      if (group) {
        if (group.layout) group.setLayout(resize(group.layout, id, ratio));
        return;
      }
      updateWorkspace(active.id, (w) => ({
        ...w,
        layout: w.layout ? resize(w.layout, id, ratio) : null,
      }));
    },
  };
}
