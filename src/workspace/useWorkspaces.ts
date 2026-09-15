import { useCallback, useEffect, useRef, useState } from "react";
import { contains, leaf, remove, resize, split, uid } from "../layout.ts";
import { initialWorkspace } from "../workspaceState.ts";
import type { ClosedProject, Routine, Saved } from "../workspaceState.ts";
import { applyChatEvent, startUserTurn } from "../chat-threads.ts";
import { disposeTerminal } from "../TerminalPanel.tsx";
import { errorText } from "../app/errors.ts";
import { agentTitle } from "../app/agent-title.ts";
import { normalizeRemote } from "../app/useProjectGit.ts";
import {
  closedProjectId,
  forgetProject as removeClosedProject,
  rememberProject,
} from "../app/projects.ts";
import {
  appendPanel,
  fixSelection,
  groupPanelIds,
  movePanel as moveInLayout,
  removeClosedPanels,
  removePanel,
  retitleTerminal,
  tidyGroupLayout,
  tidyWorkspace,
} from "./workspace-actions.ts";
import type { GroupCanvasContext } from "./workspace-actions.ts";
import type { ModelProfile, Panel, PanelKind, Workspace } from "../types";

export type WorkspaceController = ReturnType<typeof useWorkspaces>;

/** Owns everything about the panel and workspace lifecycle. The workspace list
 * itself lives in App because the Herdr controller writes into it too; this hook
 * and that one are its only mutators. */
export function useWorkspaces({
  workspaces,
  setWorkspaces,
  saved,
  socket,
  refreshHerdr,
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
  const [closedProjects, setClosedProjects] = useState<ClosedProject[]>(
    saved?.closedProjects || [],
  );
  const active = workspaces.find((w) => w.id === activeId) || workspaces[0];
  const activeRef = useRef(active);
  const workspacesRef = useRef(workspaces);
  activeRef.current = active;
  workspacesRef.current = workspaces;
  const zoomedRef = useRef(zoomed);
  const dragIdRef = useRef(dragId);
  zoomedRef.current = zoomed;
  dragIdRef.current = dragId;
  // The active workspace's merge group (flat mode only), kept in a ref
  // rather than a hook parameter: it depends on git identity data (see
  // useProjectGit) fetched from App, which in turn needs this hook's own
  // `active` workspace - App assigns it here, synchronously, right after
  // calling this hook each render (src/workspace/mergedLayouts.ts).
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
      const current = activeRef.current;
      const panel = current.panels.find((item) => item.id === panelId);
      if (!panel) return;
      if (panel.herdrId) {
        const endpoint = current.connection || socket;
        window.bridge
          ?.herdr(endpoint, "pane.rename", {
            pane_id: panel.herdrId,
            label: title,
          })
          .then(() => refreshHerdr(endpoint))
          .catch((error) => notify(errorText(error)));
      } else {
        updatePanel(panelId, { title });
      }
    },
    [notify, refreshHerdr, socket, updatePanel],
  );
  /** Places a panel App already built (an extension surface) into the active
   * workspace. */
  /** "Hide only": drop the pane from the layout but leave its process running.
   * The Herdr session and the transcript are untouched. */
  function hidePanel(workspaceId: string, panelId: string) {
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      layout: remove(w.layout, panelId),
    }));
    window.bridge?.terminalClose(panelId);
    disposeTerminal(panelId);
    setZoomed(null);
  }
  /** Renames a workspace, telling Herdr first when the workspace is one of its
   * sessions so the two names never drift. */
  async function renameWorkspace(workspaceId: string, name: string) {
    const workspace = workspacesRef.current.find((w) => w.id === workspaceId);
    if (!workspace) return;
    if (workspace.herdrId)
      await window.bridge!.herdr(
        workspace.connection || socket,
        "workspace.rename",
        { workspace_id: workspace.herdrId, label: name },
      );
    updateWorkspace(workspaceId, (w) => ({ ...w, name }));
  }
  /** Creates a workspace either as a Herdr session or a local one. Plugins
   * are configured afterwards from the workspace's own controls. Returns false
   * when nothing was created. */
  async function createWorkspace(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    endpoint: string = socket,
  ): Promise<boolean> {
    try {
      if (backend === "herdr") {
        const result = await window.bridge!.herdr(
          endpoint,
          "workspace.create",
          { label: name, cwd, focus: false },
        );
        await refreshHerdr(endpoint);
        if (starter !== "shell")
          await window.bridge!.herdr(endpoint, "pane.send_input", {
            pane_id: result.root_pane.pane_id,
            text: starter,
            keys: ["Enter"],
          });
        await refreshHerdr(endpoint);
        switchWorkspace(
          `herdr:${endpoint.startsWith("ssh:") ? endpoint : "local"}:${result.workspace.workspace_id}`,
        );
      } else {
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
      }
      return true;
    } catch (error) {
      notify(errorText(error));
      return false;
    }
  }
  function insertPanel(panel: Panel) {
    const current = activeRef.current;
    updateWorkspace(current.id, (w) => appendPanel(w, panel));
    setSelected(panel.id);
    showWorkspace();
    setZoomed(null);
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
    backend?: "herdr" | "local",
    targetWorkspaceId?: string,
  ) {
    const modelProfileId = modelProfile?.id;
    // A merged-row session host choice (D2): defaults to the active
    // workspace, same as before targetWorkspaceId existed.
    const current =
      (targetWorkspaceId &&
        workspacesRef.current.find((w) => w.id === targetWorkspaceId)) ||
      activeRef.current;
    if (adding) return;
    setAdding(true);
    try {
      // A Herdr workspace runs its sessions in Herdr unless this panel was
      // asked to be local. Nothing else can start a process.
      const viaHerdr = current.herdrId && (backend ?? "herdr") === "herdr";
      if (viaHerdr && (kind === "terminal" || kind === "agent")) {
        if (!window.bridge) throw new Error("Open the desktop app first.");
        const endpoint = current.connection || socket;
        // Staged before the pane exists: a gateway that can't be reached
        // shouldn't leave an empty pane behind. Herdr panes are just a typed
        // shell command, so the model swap rides on that command line
        // instead of the argv/env injection the direct local launch uses.
        let launchText = agent;
        if (kind === "agent" && agent === "claude" && modelProfileId) {
          if (endpoint.startsWith("ssh:"))
            throw new Error(
              "Custom models aren't supported on remote (SSH) workspaces yet.",
            );
          const settingsPath =
            await window.bridge.modelSettingsStage(modelProfileId);
          launchText = `claude --settings '${settingsPath}'`;
        }
        const result = await window.bridge.herdr(endpoint, "pane.split", {
          workspace_id: current.herdrId,
          target_pane_id: current.panels.find((p) => p.herdrId)?.herdrId,
          direction: "right",
          focus: false,
          cwd: current.cwd,
        });
        const paneId = result.pane?.pane_id || result.pane_id;
        if (kind === "agent" && paneId)
          await window.bridge.herdr(endpoint, "pane.send_input", {
            pane_id: paneId,
            text: launchText,
            keys: ["Enter"],
          });
        await refreshHerdr(endpoint);
        if (paneId)
          setSelected(
            `herdr:${endpoint.startsWith("ssh:") ? endpoint : "local"}:${paneId}`,
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
    } catch (error) {
      notify(errorText(error));
    } finally {
      setAdding(false);
    }
  }
  const closePanel = useCallback(
    (panelId: string) => {
      const owner = workspacesRef.current.find((workspace) =>
        workspace.panels.some((panel) => panel.id === panelId),
      );
      const panel = owner?.panels.find((item) => item.id === panelId);
      if (!owner || !panel) return;
      if (panel.herdrId) {
        confirmClose({ workspace: owner, panel });
        return;
      }
      disposeTerminal(panel.id);
      // Closing an imported panel hides it locally; it never kills a Herdr process.
      if (!panel.herdrId)
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
    [notify, updateWorkspace],
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

  async function endSessions(items: { workspace: Workspace; panel: Panel }[]) {
    const closed = new Set<string>(),
      errors: string[] = [];
    for (const { workspace, panel } of items) {
      try {
        if (panel.herdrId)
          await window.bridge!.herdr(
            workspace.connection || socket,
            "pane.close",
            { pane_id: panel.herdrId },
          );
        else await window.bridge?.terminalClose(panel.id);
        if (panel.busy) await window.bridge?.cancelChat(panel.id);
        await window.bridge?.terminalClose(panel.id);
        disposeTerminal(panel.id);
        closed.add(panel.id);
      } catch (error) {
        errors.push(panel.title + ": " + errorText(error));
      }
    }
    setWorkspaces((list) => removeClosedPanels(list, closed));
    if (zoomed && closed.has(zoomed)) setZoomed(null);
    for (const endpoint of new Set(
      items
        .filter((i) => i.panel.herdrId)
        .map((i) => i.workspace.connection || socket),
    ))
      await refreshHerdr(endpoint);
    if (errors.length) notify(errors.join("; "));
  }
  async function endWorkspace(workspace: Workspace) {
    try {
      if (workspace.herdrId)
        await window.bridge!.herdr(
          workspace.connection || socket,
          "workspace.close",
          { workspace_id: workspace.herdrId },
        );
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
    }
  }
  /** Keeps the just-closed workspace on the Dashboard (B1): a fresh git
   * identity read the same way `useProjectGit` reads one, since the
   * workspace is about to disappear and can no longer be looked up by id. */
  function rememberClosed(workspace: Workspace) {
    const endpoint = workspace.herdrId
      ? workspace.connection || socket
      : undefined;
    const entry: ClosedProject = {
      id: closedProjectId(endpoint, workspace.cwd),
      name: workspace.name,
      cwd: workspace.cwd,
      endpoint,
      herdr: Boolean(workspace.herdrId),
      closedAt: Date.now(),
      git: { remote: "", commonDir: "", checkout: "", subdir: "", branch: "" },
    };
    setClosedProjects((list) => rememberProject(list, entry));
    window.bridge
      ?.projectInspect(workspace.connection, {
        operation: "git_remote",
        root: workspace.cwd,
      })
      .then((result) =>
        setClosedProjects((list) =>
          rememberProject(list, {
            ...entry,
            git: {
              remote: normalizeRemote(result?.remote || ""),
              commonDir: result?.commonDir || "",
              checkout: result?.checkout || "",
              subdir: result?.subdir || "",
              branch: result?.branch || "",
            },
          }),
        ),
      )
      .catch(() => {});
  }
  /** Reopens a remembered project through the path a fresh workspace already
   * uses - Herdr on its original host when it had one, else local - and
   * forgets it only once that succeeds, so a host that cannot be reached
   * (surfaced via `notify` inside `createWorkspace`) leaves the entry in
   * place to retry. */
  async function reopenProject(project: ClosedProject) {
    const ok = await createWorkspace(
      project.name,
      project.cwd,
      project.herdr ? "herdr" : "local",
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
    focusPanel,
    startPanelDrag,
    endPanelDrag,
    zoomPanel,
    startPanel,
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
