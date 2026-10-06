import { Suspense, useCallback, useEffect, useState } from "react";
import { X } from "lucide-react";
import type { Panel, Workspace } from "./types";
import { uid } from "./layout";
import { ChatView } from "./ChatView";
import { AgentsView } from "./agents/AgentsView";
import { ProjectSettingsDialog } from "./ProjectSettingsDialog";
import { WorkspaceDialog } from "./WorkspaceDialog";
import { errorText } from "./app/errors";
import { useAppPersistence } from "./app/useAppPersistence";
import { useSessionState } from "./app/useSessionState";
import { useAttention } from "./app/useAttention";
import { useCompact } from "./app/useCompact";
import { useKeepAwake } from "./app/useKeepAwake";
import { useAgentNotices } from "./app/useAgentNotices";
import { useKeyboardShortcuts } from "./app/useKeyboardShortcuts";
import { PanelPickerDialog } from "./app/PanelPickerDialog";
import { useWorkspaces } from "./workspace/useWorkspaces";
import { WorkspaceCanvas } from "./workspace/WorkspaceCanvas";
import { CloseSessionDialog } from "./dialogs/CloseSessionDialog";
import { DialogHost } from "./dialogs/DialogHost";
import { resolveDialog, type Dialog } from "./dialogs/dialog-state";
import { UpdateSettings } from "./dialogs/lazy-settings";
import { NotificationsDialog } from "./app/NotificationsDialog";
import { RoutineDialog } from "./app/RoutineDialog";
import { useOpenSignals } from "./extensions/useOpenSignals.ts";
import { SettingsDialog, useSettingsTab } from "./app/SettingsDialog";
import { SectionPage } from "./app/SectionPage";
import { Sidebar } from "./app/Sidebar";
import { TitleBar } from "./app/TitleBar";
import { useExtensions } from "./app/useExtensions";
import { useConnectionProfiles } from "./app/useConnectionProfiles";
import { useDaemon } from "./app/useDaemon";
import { useHostContext, useProjectGit } from "./app/useProjectGit";
import { useMergedCanvas } from "./workspace/mergedLayouts";
import { useProjectView } from "./workspace/projectView";
import { useSkills } from "./app/useSkills";
import { useToast } from "./app/useToast";
import { useOrchestratorNotices } from "./orchestrator/useOrchestratorNotices";
import { useUpdates } from "./app/useUpdates";
import {
  activePage,
  resolveNavigation,
  resolvePaneOpen,
  primaryNavigation,
  resolveExtensionCommand,
} from "./extensions/routes";
import type { ExtensionRouteState } from "./app/navigation";
import { useAppNavigation } from "./app/useAppNavigation";
import {
  codePanels,
  initialWorkspace,
  restore,
  type Routine,
} from "./workspaceState";

const saved = restore();

export function App() {
  const { toast, setToast, notify } = useToast();
  const updates = useUpdates(notify);
  const { notices: agentNotices, setNotices: setAgentNotices } =
    useAgentNotices(notify);
  const {
    registry: extensionRegistry,
    snapshot: extensionSnapshot,
    loaded: extensionsLoaded,
    setEnabled: setExtensionEnabled,
    refresh: refreshExtensions,
  } = useExtensions(notify);
  const [workspaces, setWorkspaces] = useState<Workspace[]>(
    saved?.workspaces || [initialWorkspace()],
  );
  const {
    mode,
    section,
    sectionName,
    route,
    setMode,
    toggleCoreSection,
    openExtension,
    showWorkspace,
  } = useAppNavigation(saved, {
    ready: extensionsLoaded,
    hasSurface: useCallback(
      (target: ExtensionRouteState) =>
        Boolean(activePage(extensionRegistry, target)),
      [extensionRegistry, extensionSnapshot],
    ),
  });
  const { connectionProfiles, refreshConnectionProfiles } =
    useConnectionProfiles();
  const {
    system,
    socket,
    setSocket,
    connection,
    connectionError,
    refreshHerdr,
    statusByEndpoint,
  } = useDaemon({
    savedSocket: saved?.socket || "",
    notify,
    setWorkspaces,
    connectionProfiles,
  });
  const skills = useSkills(sectionName, notify);
  const [sidebar, setSidebar] = useState(
    saved?.sidebar ?? window.innerWidth >= 760,
  );
  const [workspaceGrouping, setWorkspaceGrouping] = useState<
    "grouped" | "flat"
  >(saved?.workspaceGrouping || "grouped");
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  useEffect(() => {
    const query = window.matchMedia("(max-width: 760px)");
    const change = () => setSidebar(!query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  const ws = useWorkspaces({
    workspaces,
    setWorkspaces,
    saved,
    socket,
    refreshHerdr,
    useEndpoint: setSocket,
    notify,
    showWorkspace,
    confirmClose: ({ workspace, panel }) =>
      setDialog({
        kind: "close-session",
        workspaceId: workspace.id,
        panelId: panel.id,
      }),
  });
  const {
    active,
    selected,
    zoomed,
    dragId,
    adding,
    setSelected,
    setZoomed,
    updatePanel,
    closePanel,
    showPanel,
    sendChat,
    newThread,
    deleteThread,
    runRoutine,
    endSessions,
    endWorkspace,
  } = ws;
  function switchWorkspace(id: string) {
    if (window.innerWidth < 760) setSidebar(false);
    ws.switchWorkspace(id);
  }
  const attention = useAttention({
    workspaces,
    connectionProfiles,
    section,
    active,
    selected,
    zoomed,
    switchWorkspace,
    setSelected,
    setZoomed,
  });
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const closeDialog = useCallback(() => setDialog(null), []);
  const [settingsTab, setSettingsTab, openConnections] =
    useSettingsTab(setDialog);
  const target = resolveDialog(dialog, workspaces);
  const [workspaceQuery, setWorkspaceQuery] = useState("");
  const [compact, canvasRef] = useCompact();
  const [routines, setRoutines] = useState<Routine[]>(saved?.routines || []);
  const [fontScale, setFontScale] = useState(saved?.fontScale || 1);
  // Held here, not in SettingsDialog: the blocker runs whenever the app is open.
  const [keepAwake, setKeepAwake] = useKeepAwake();
  const connected = connection === "connected";
  const activeEndpoint = active.connection || socket;
  const { projectGit, readyWorkspaceIds, hydratedHostKeys } = useProjectGit(
    workspaces,
    active.id,
    ws.setProjectGit,
  );
  const session = useSessionState(saved);
  const merged = useMergedCanvas(
    ws,
    projectGit,
    connectionProfiles,
    workspaceGrouping,
    socket,
    session,
  );
  const { tabMode, setTabMode, views } = useProjectView(
    merged.group?.id ?? active.id,
    ws,
    saved,
  );
  const orchestrator = useOrchestratorNotices({
    workspaces,
    showWorkspace,
    switchWorkspace,
    setSelected,
    setZoomed,
    addPanel: ws.addPanel,
    createWorkspace: ws.createWorkspace,
  });
  const hostContext = useHostContext(
    { workspaces, projectGit, connectionProfiles, workspaceGrouping },
    ws.createWorkspace,
    closeDialog,
  );
  function selectHostPane(workspace: Workspace, panel: Panel) {
    switchWorkspace(workspace.id);
    setSelected(panel.id);
    setZoomed(panel.id);
  }
  const primaryExtensionNavigation = primaryNavigation(extensionRegistry);
  const openPanelPicker = useCallback(() => setDialog({ kind: "pane" }), []);
  useAppPersistence(
    {
      workspaces,
      activeId: active.id,
      socket,
      routines,
      fontScale,
      mode,
      tabMode,
      section: sectionName,
      selected,
      zoomed,
      sidebar,
      route,
      workspaceGrouping,
      closedProjects: ws.closedProjects,
      views,
      ...session,
    },
    notify,
  );

  useOpenSignals(workspaces, extensionRegistry, ws, notify);
  function addExtensionPanel(
    extensionId: string,
    contributionId: string,
    targetWorkspaceId?: string,
  ) {
    if (adding) return;
    try {
      const open = resolvePaneOpen(extensionRegistry, active.panels, {
        extensionId,
        surfaceId: contributionId,
      });
      if (open.kind === "unavailable") return notify(open.reason);
      if (open.kind === "show") {
        showPanel(open.panel);
        closeDialog();
        return;
      }
      const panel = extensionRegistry.createPanel(
        extensionId,
        contributionId,
        uid(),
      );
      ws.insertPanel(panel, targetWorkspaceId);
      closeDialog();
    } catch (error) {
      notify(errorText(error));
    }
  }
  /** Opens whatever a navigation entry points at: an app page, a workspace
   * pane, or a pane shown as a tab. The surface decides, not the button. */
  function openExtensionTarget(extensionId: string, targetSurfaceId: string) {
    const target = resolveNavigation(extensionRegistry, {
      extensionId,
      targetSurfaceId,
    });
    if (target.kind === "unavailable") return notify(target.reason);
    if (target.kind === "page") {
      openExtension({
        extensionId: target.extensionId,
        surfaceId: target.surfaceId,
      });
      setZoomed(null);
      return;
    }
    if (target.kind === "tab") setTabMode(true);
    addExtensionPanel(target.extensionId, target.surfaceId);
  }
  function runExtensionCommand(extensionId: string, commandId: string) {
    const target = resolveExtensionCommand(
      extensionRegistry,
      extensionSnapshot,
      extensionId,
      commandId,
    );
    if (target.kind === "unavailable") return notify(target.reason);
    if (target.kind === "pane")
      return addExtensionPanel(target.extensionId, target.surfaceId);
    openExtension({
      extensionId: target.extensionId,
      surfaceId: target.surfaceId,
    });
    setZoomed(null);
  }

  useKeyboardShortcuts({
    active,
    selected,
    dialogOpen: Boolean(dialog),
    mode,
    section: Boolean(section),
    openPanelPicker: () => setDialog({ kind: "pane" }),
    closeDialog,
    setZoomed,
    setSelected,
    setSidebar,
    closePanel,
  });
  const totalPanels = workspaces.reduce(
    (sum, w) => sum + codePanels(w).length,
    0,
  );
  const paneCount = merged.group
    ? merged.panes.length
    : codePanels(active).length;
  const useTabs = tabMode || compact || paneCount > 6;

  return (
    <div
      className={`app ${compact ? "compact" : ""}`}
      style={{ fontSize: `${13 * fontScale}px` }}
    >
      <TitleBar
        nav={{
          mode,
          section: Boolean(section),
          currentRouteId: route.surfaceId,
          setMode,
          openExtension,
          showWorkspace,
        }}
        active={active}
        updates={updates}
        tabMode={tabMode}
        setTabMode={setTabMode}
        sidebar={sidebar}
        setSidebar={setSidebar}
        setZoomed={setZoomed}
        openDialog={(kind) => setDialog({ kind })}
        primaryExtensionNavigation={primaryExtensionNavigation}
        extensionRegistry={extensionRegistry}
        runExtensionCommand={runExtensionCommand}
        openFiles={() => {
          const panel = active.panels.find((p) => p.kind === "files");
          panel ? showPanel(panel) : ws.addPanel("files");
        }}
        tidy={() => {
          ws.tidy();
          setZoomed(null);
          showWorkspace();
          setTabMode(false);
        }}
        noticeCount={agentNotices.length}
      />
      <div className="app-body">
        {sidebar && (
          <Sidebar
            worktreeTasks={orchestrator.worktreeTasks}
            registry={extensionRegistry}
            openExtensionTarget={openExtensionTarget}
            runExtensionCommand={runExtensionCommand}
            mode={mode}
            onWorkspace={!section}
            currentRouteId={route.surfaceId}
            inboxCount={attention.waiting}
            toggleCoreSection={toggleCoreSection}
            openDialog={(kind) => setDialog({ kind })}
            manageWorkspace={(workspace) =>
              setDialog({
                kind: "workspace-actions",
                workspaceId: workspace.id,
              })
            }
            openSettings={() => setDialog({ kind: "settings" })}
            workspaces={workspaces}
            active={active}
            workspaceQuery={workspaceQuery}
            setWorkspaceQuery={setWorkspaceQuery}
            switchWorkspace={switchWorkspace}
            showPanel={showPanel}
            selectHostPane={selectHostPane}
            requestClose={({ workspace, panel }) =>
              setDialog(
                panel
                  ? {
                      kind: "close-session",
                      workspaceId: workspace.id,
                      panelId: panel.id,
                    }
                  : { kind: "workspace-actions", workspaceId: workspace.id },
              )
            }
            setSlot={setSlot}
            selected={selected}
            connected={connected}
            connection={connection}
            localSocket={system?.socketPath || ""}
            connectionProfiles={connectionProfiles}
            statusByEndpoint={statusByEndpoint}
            projectGit={projectGit}
            readyWorkspaceIds={readyWorkspaceIds}
            hydratedHostKeys={hydratedHostKeys}
            workspaceGrouping={workspaceGrouping}
            setWorkspaceGrouping={setWorkspaceGrouping}
            totalPanels={totalPanels}
            notify={notify}
          />
        )}
        <main
          ref={canvasRef}
          className={`workspace-canvas ${dragId ? "is-dragging" : ""} ${useTabs ? "tabbed-canvas" : ""}`}
        >
          {section ? (
            <SectionPage
              registry={extensionRegistry}
              openExtensionTarget={openExtensionTarget}
              cwd={active.cwd}
              connection={active.connection}
              section={section}
              runExtensionCommand={runExtensionCommand}
              workspaces={workspaces}
              routines={routines}
              setRoutines={setRoutines}
              runRoutine={runRoutine}
              skills={skills}
              extensionSnapshot={extensionSnapshot}
              notify={notify}
              connected={connected}
              activeEndpoint={activeEndpoint}
              home={system?.home}
              switchWorkspace={switchWorkspace}
              openOrchestratorTask={orchestrator.openTask}
              addExtensionPanel={addExtensionPanel}
              setExtensionEnabled={(extensionId, enabled) =>
                void setExtensionEnabled(extensionId, enabled)
              }
              refreshExtensions={() => void refreshExtensions()}
              openRoutineDialog={() => setDialog({ kind: "routine" })}
              totalPanels={totalPanels}
              ws={ws}
              projectGit={projectGit}
              connectionProfiles={connectionProfiles}
              attention={attention}
            />
          ) : mode === "Agent" ? (
            <AgentsView slot={slot} session={session} />
          ) : mode === "Chat" ? (
            <ChatView
              workspaces={workspaces}
              active={active}
              slot={slot}
              onSelectWorkspace={ws.selectWorkspace}
              onNewThread={newThread}
              onSend={sendChat}
              onCancel={(id) =>
                window.bridge
                  ?.cancelChat(id)
                  .catch((error) => notify(errorText(error)))
              }
              onPatch={updatePanel}
              onDelete={deleteThread}
              onToggleSidebar={() => setSidebar(!sidebar)}
              session={session}
            />
          ) : (
            <WorkspaceCanvas
              ws={ws}
              activeEndpoint={activeEndpoint}
              extensionRegistry={extensionRegistry}
              tabMode={tabMode}
              compact={compact}
              openPanelPicker={openPanelPicker}
              openConnections={openConnections}
              merged={merged}
            />
          )}
        </main>
      </div>
      {toast && (
        <div className="toast" role="status">
          <span>{toast}</span>
          <button aria-label="Dismiss message" onClick={() => setToast("")}>
            <X size={14} />
          </button>
        </div>
      )}
      <DialogHost dialog={dialog} onClose={closeDialog}>
        {dialog &&
          (dialog.kind === "updates" ? (
            <Suspense
              fallback={<div className="loading">Loading updates…</div>}
            >
              <UpdateSettings state={updates} />
            </Suspense>
          ) : dialog.kind === "close-session" && target?.panel ? (
            <CloseSessionDialog
              workspace={target.workspace}
              panel={target.panel}
              workspaces={workspaces}
              projectGit={projectGit}
              hidePanel={ws.hidePanel}
              endSessions={endSessions}
              onClose={closeDialog}
            />
          ) : dialog.kind === "workspace-actions" && target ? (
            <ProjectSettingsDialog
              workspace={target!.workspace}
              workspaces={workspaces}
              onEndWorkspace={endWorkspace}
              onRename={(name) =>
                ws.renameWorkspace(target!.workspace.id, name)
              }
              onCloseWorkspace={async () => {
                await endWorkspace(target!.workspace);
                closeDialog();
              }}
            />
          ) : dialog.kind === "pane" ? (
            <PanelPickerDialog
              active={active}
              adding={adding}
              system={system}
              addPanel={async (...args) => {
                await ws.addPanel(...args);
                closeDialog();
              }}
              addExtensionPanel={addExtensionPanel}
              extensionRegistry={extensionRegistry}
              connected={connected}
              hostContext={hostContext}
            />
          ) : dialog.kind === "settings" ? (
            <SettingsDialog
              registry={extensionRegistry}
              cwd={active.cwd}
              connection={active.connection}
              workspaces={workspaces}
              settingsTab={settingsTab}
              setSettingsTab={setSettingsTab}
              socket={socket}
              setSocket={setSocket}
              connected={connected}
              connectionError={connectionError}
              refreshHerdr={refreshHerdr}
              fontScale={fontScale}
              setFontScale={setFontScale}
              keepAwake={keepAwake}
              setKeepAwake={setKeepAwake}
              updates={updates}
              system={system}
              connectionProfiles={connectionProfiles}
              refreshConnectionProfiles={refreshConnectionProfiles}
              notify={notify}
            />
          ) : dialog.kind === "workspace" ? (
            <WorkspaceDialog
              defaultCwd={active.cwd}
              activeEndpoint={active.connection}
              localSocket={system?.socketPath || ""}
              connectionProfiles={connectionProfiles}
              statusByEndpoint={statusByEndpoint}
              onCreate={(...args) => ws.createWorkspace(...args)}
              onStart={() => (closeDialog(), openPanelPicker())}
              onClose={closeDialog}
            />
          ) : dialog.kind === "routine" ? (
            <RoutineDialog
              setRoutines={setRoutines}
              close={() => closeDialog()}
            />
          ) : (
            <NotificationsDialog
              agentNotices={agentNotices}
              setAgentNotices={setAgentNotices}
              updates={updates}
              openUpdates={() => setDialog({ kind: "updates" })}
            />
          ))}
      </DialogHost>
    </div>
  );
}
