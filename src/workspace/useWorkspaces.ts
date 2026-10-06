import { useCallback, useEffect, useRef, useState } from "react";
import { contains, leaf, remove, resize, split, uid } from "../layout.ts";
import { initialWorkspace } from "../workspaceState.ts";
import type { ClosedProject, Routine, Saved } from "../workspaceState.ts";
import { applyChatEvent, startUserTurn } from "../chat-threads.ts";
import { disposeTerminal } from "../TerminalPanel.tsx";
import { errorText, isGone } from "../app/errors.ts";
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
  removeClosedSessions,
  removePanel,
  renameRequest,
  reopenRequest,
  retitleTerminal,
  tidyGroupLayout,
  tidyWorkspace,
} from "./workspace-actions.ts";
import type { GroupCanvasContext } from "./workspace-actions.ts";
import {
  herdrWorkspaceKey,
  launchesInWorktree,
  worktreeBranchError,
} from "./worktree.ts";
import { applySessionLaunch, findSessionWorkspace } from "./session-launch.ts";
import { hostOf } from "../projectWorktrees.ts";
import { sessionPanelId } from "../daemonSessions.ts";
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
  socket,
  refreshHerdr,
  invalidateHerdr,
  useEndpoint,
  notify,
  showWorkspace,
  confirmClose,
}: {
  workspaces: Workspace[];
  setWorkspaces: React.Dispatch<React.SetStateAction<Workspace[]>>;
  saved: Saved | null;
  socket: string;
  refreshHerdr(path: string): Promise<void>;
  invalidateHerdr?(path: string): void;
  useEndpoint(endpoint: string): void;
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
  const failedLaunches = useRef(new Map<string, SessionLaunchRequest>());
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
    (panelId: string, patch: Partial<Omit<Panel, "kind" | "extension">>) =>
      setWorkspaces((items) =>
        items.map((w) => ({
          ...w,
          panels: w.panels.map((p) =>
            p.id === panelId ? ({ ...p, ...patch } as Panel) : p,
          ),
        })),
      ),
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
  const startPanel = useCallback(
    (panelId: string) => updatePanel(panelId, { started: true }),
    [updatePanel],
  );
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
  useEffect(
    () =>
      window.bridge?.onTerminal((event) => {
        if (event.agent !== undefined)
          setWorkspaces((items) =>
            items.map((w) => ({
              ...w,
              panels: w.panels.map((p) =>
                p.id === event.panelId ? retitleTerminal(p, event.agent) : p,
              ),
            })),
          );
      }),
    [],
  );
  const renamePanel = useCallback(
    (panelId: string, title: string) => {
      const owner = findPanelOwner(workspacesRef.current, panelId);
      const panel = owner?.panels.find((item) => item.id === panelId);
      if (!owner || !panel) return;
      const request = renameRequest(owner, panel, title, socket);
      if (request) {
        window.bridge
          ?.sessionUpdate(request.host, request.patch)
          .then(() => updatePanel(panelId, { title }))
          .catch((error) => notify(errorText(error)));
      } else {
        updatePanel(panelId, { title });
      }
    },
    [notify, socket, updatePanel],
  );
  /** "Hide only": drop the pane from the layout but leave its process running.
   * The session and the transcript are untouched. */
  function hidePanel(workspaceId: string, panelId: string) {
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      layout: remove(w.layout, panelId),
    }));
    window.bridge?.terminalClose(panelId);
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
  ): Promise<boolean> {
    const operationKey = JSON.stringify([
      request.endpoint,
      request.operationId,
    ]);
    launches.current += 1;
    setAdding(true);
    invalidateHerdr?.(request.endpoint);
    try {
      if (!window.bridge) throw new Error("Open the desktop app first.");
      const result = await window.bridge.sessionLaunch(request);
      const value = result.ok ? result.value : result.error.created;
      if (value) {
        const panel: Panel = {
          ...template,
          id: sessionPanelId(request.endpoint, value.paneId),
          sessionId: value.paneId,
          started: result.ok && request.kind === "agent",
          launchOperationId: result.ok ? undefined : request.operationId,
          launchError: result.ok ? undefined : result.error.message,
        };
        delete panel.ended;
        invalidateHerdr?.(request.endpoint);
        setWorkspaces((items) =>
          applySessionLaunch(
            items,
            request.endpoint,
            value,
            panel,
            restore,
            request.label,
            request.worktree?.branch,
          ),
        );
        switchWorkspace(
          restore?.workspaceId ||
            findSessionWorkspace(workspacesRef.current, request.endpoint, value)
              ?.id ||
            herdrWorkspaceKey(request.endpoint, value.workspaceId),
        );
        setSelected(panel.id);
        if (restore) {
          disposeTerminal(restore.panelId);
          window.bridge.terminalClose(restore.panelId).catch(() => {});
          if (zoomedRef.current === restore.panelId) setZoomed(panel.id);
        }
      }
      if (!result.ok) {
        if (result.error.retryable)
          failedLaunches.current.set(operationKey, request);
        else failedLaunches.current.delete(operationKey);
        notify(result.error.message);
        return false;
      }
      failedLaunches.current.delete(operationKey);
      await refreshHerdr(request.endpoint).catch((error) =>
        notify(errorText(error)),
      );
      return true;
    } catch (error) {
      failedLaunches.current.set(operationKey, request);
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
    backend: string,
    starter: string,
    endpoint: string = socket,
    operationId?: string,
    env?: Record<string, string>,
  ): Promise<boolean> {
    if (backend === "herdr") {
      const kind = starter === "shell" ? "terminal" : "agent";
      const pending = workspacesRef.current
        .flatMap((workspace) =>
          workspace.connection === endpoint && workspace.cwd === cwd
            ? workspace.panels
            : [],
        )
        .find(
          (panel) =>
            panel.launchError &&
            panel.launchOperationId &&
            operationId &&
            panel.launchOperationId === operationId &&
            panel.kind === kind &&
            panel.agent === (kind === "agent" ? starter : undefined),
        );
      const owner =
        pending && findPanelOwner(workspacesRef.current, pending.id);
      const request: SessionLaunchRequest = {
        operationId: operationId || pending?.launchOperationId || uid(),
        endpoint,
        cwd,
        label: name,
        kind,
        agent: kind === "agent" ? starter : undefined,
        ...(pending && owner
          ? { workspaceId: owner.id, paneId: pending.sessionId }
          : {}),
        env,
      };
      return launchSession(
        request,
        {
          id: "",
          kind,
          title: kind === "terminal" ? "zsh" : agentTitle(starter),
          agent: kind === "agent" ? starter : undefined,
        },
        undefined,
      );
    }
    const w = initialWorkspace(cwd);
    w.name = name;
    const panel: Panel = {
      id: uid(),
      kind: starter === "shell" ? "terminal" : "agent",
      title: starter === "shell" ? "zsh" : agentTitle(starter),
      agent: starter === "shell" ? undefined : starter,
      started: starter !== "shell",
    };
    w.panels = [panel];
    w.layout = leaf(panel.id);
    setWorkspaces((items) => [...items, w]);
    switchWorkspace(w.id);
    return true;
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
    const target = workspacesRef.current.find((w) => w.id === id);
    // eslint-disable-next-line react-hooks/rules-of-hooks -- useEndpoint is a plain callback prop (App.tsx passes setSocket), not a hook; the name predates this lint rule
    if (target?.connection) useEndpoint(target.connection);
    setActiveId(id);
  }
  function switchWorkspace(id: string) {
    const target = workspaces.find((w) => w.id === id);
    // eslint-disable-next-line react-hooks/rules-of-hooks -- same non-hook callback, see selectWorkspace above
    if (target?.connection) useEndpoint(target.connection);
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
    backend?: "herdr" | "local",
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
      // A host-backed workspace runs its sessions in the daemon unless this
      // panel was asked to be local. Nothing else can start a process.
      const viaHerdr = current.connection && (backend ?? "herdr") === "herdr";
      if (prompt && !(viaHerdr && launchesInWorktree(kind)))
        throw new Error("Starting with a prompt needs a host workspace.");
      if (viaHerdr && launchesInWorktree(kind)) {
        const endpoint = current.connection || socket;
        const pending = current.panels.find(
          (panel) =>
            panel.launchError &&
            panel.launchOperationId &&
            operationId &&
            panel.launchOperationId === operationId &&
            panel.kind === kind &&
            panel.agent === (kind === "agent" ? agent : undefined) &&
            panel.modelProfileId === modelProfileId,
        );
        return await launchSession(
          {
            operationId: operationId || pending?.launchOperationId || uid(),
            endpoint,
            cwd: current.cwd,
            label: worktree
              ? `${current.name} · ${worktree.branch}`
              : current.name,
            kind: kind === "agent" ? "agent" : "terminal",
            agent: kind === "agent" ? agent : undefined,
            modelProfileId,
            ...accounts,
            env,
            workspaceId: current.id,
            ...(pending ? { paneId: pending.sessionId } : {}),
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
      } else if (worktree && launchesInWorktree(kind)) {
        // The local counterpart of the branch above: no daemon involved, so
        // the new checkout runs as a plain local process on this Mac (also
        // reached from a host workspace whose picker backend was "local").
        if (!window.bridge) throw new Error("Open the desktop app first.");
        const branchError = worktreeBranchError(worktree.branch);
        if (branchError) throw new Error(branchError);
        const { path } = await window.bridge.worktreeCreate(
          current.cwd,
          worktree.branch,
          worktree.base,
        );
        const w = initialWorkspace(path);
        w.localWorktree = true;
        w.worktreeBranch = worktree.branch;
        w.name = `${current.name} · ${worktree.branch}`;
        const panel: Panel = {
          id: uid(),
          kind,
          title:
            kind === "agent" ? modelProfile?.label || agentTitle(agent) : "zsh",
          agent: kind === "agent" ? agent : undefined,
          started: kind === "agent",
          modelProfileId: kind === "agent" ? modelProfileId : undefined,
          ...accounts,
        };
        w.panels = [panel];
        w.layout = leaf(panel.id);
        setWorkspaces((items) => [...items, w]);
        switchWorkspace(w.id);
        return true;
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
                    : kind === "orchestrator"
                      ? "Orchestrator"
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
      (!ended.ended && !ended.launchError)
    )
      return Promise.resolve();
    const endpoint = owner.connection || socket;
    const cached = ended.launchOperationId
      ? failedLaunches.current.get(
          JSON.stringify([endpoint, ended.launchOperationId]),
        )
      : undefined;
    const request: SessionLaunchRequest =
      cached &&
      !ended.ended &&
      (!operationId || cached.operationId === operationId)
        ? cached
        : reopenRequest(
            owner,
            ended,
            operationId || (!ended.ended && ended.launchOperationId) || uid(),
            socket,
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
            if (panel.sessionId) await closeSession(workspace, panel, socket);
            else await window.bridge?.terminalClose(panel.id);
            if (panel.busy) await window.bridge?.cancelChat(panel.id);
            await window.bridge?.terminalClose(panel.id);
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
      for (const endpoint of new Set(
        items
          .filter((i) => i.panel.sessionId)
          .map((i) => i.workspace.connection || socket),
      ))
        await refreshHerdr(endpoint);
      if (errors.length) notify(errors.join("; "));
    },
    [notify, refreshHerdr, setWorkspaces, socket],
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
      if (panel.sessionId && !panel.ended) {
        if (
          owner.panels.filter((item) => item.sessionId && !item.ended)
            .length === 1
        ) {
          const git = projectGitRef.current[owner.id];
          // No folder means no worktree to offer; an unread one may be.
          if (owner.cwd && (!git || git.linkedWorktree)) {
            confirmClose({ workspace: owner, panel });
            return;
          }
          void endSessions([{ workspace: owner, panel }]);
          return;
        }
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
      if (!panel.sessionId)
        window.bridge
          ?.terminalClose(panel.id)
          .catch((error) => notify(errorText(error)));
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
    const panel: Panel = { id: uid(), kind: "terminal", title: routine.name };
    try {
      await window.bridge.terminalOpen({
        panelId: panel.id,
        cwd: active.cwd,
        endpoint: active.connection,
      });
      await window.bridge.terminalWrite(panel.id, routine.command + "\r");
      updateWorkspace(active.id, (w) => ({
        ...w,
        panels: [...w.panels, panel],
        layout: w.layout
          ? split(w.layout, leaf(panel.id), "row")
          : leaf(panel.id),
      }));
      showWorkspace();
      setZoomed(panel.id);
    } catch (error) {
      notify(errorText(error));
    }
  }

  const closing = useRef(new Set<string>());
  async function endWorkspace(workspace: Workspace) {
    // A second click while the first is closing it does nothing.
    if (closing.current.has(workspace.id)) return;
    closing.current.add(workspace.id);
    try {
      for (const panel of workspace.panels)
        if (panel.sessionId) await closeSession(workspace, panel, socket);
      for (const panel of workspace.panels) {
        await window.bridge?.terminalClose(panel.id);
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
      project.backed ? "herdr" : "local",
      "shell",
      project.endpoint || socket,
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
