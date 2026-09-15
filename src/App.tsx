import { Suspense, useCallback, useEffect, useState } from "react";
import { X } from "lucide-react";
import type { Panel, Workspace } from "./types";
import { uid } from "./layout";
import { ChatView } from "./ChatView";
import { AgentsView } from "./agents/AgentsView";
import { SessionsDialog } from "./SessionsDialog";
import { ClaudeMcpSettings } from "./ClaudeMcpSettings";
import { WorkspaceDialog } from "./WorkspaceDialog";
import { errorText } from "./app/errors";
import { useAppPersistence } from "./app/useAppPersistence";
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
import { SettingsDialog } from "./app/SettingsDialog";
import { SectionPage } from "./app/SectionPage";
import { Sidebar } from "./app/Sidebar";
import { TitleBar } from "./app/TitleBar";
import { useExtensions } from "./app/useExtensions";
import { useConnectionProfiles } from "./app/useConnectionProfiles";
import { useHerdr } from "./app/useHerdr";
import { useProjectGit } from "./app/useProjectGit";
import { activeMergedHostLabel } from "./app/workspaceMerge";
import { useSkills } from "./app/useSkills";
import { useToast } from "./app/useToast";
import { useUpdates } from "./app/useUpdates";
import { blockedPanels, tidyWorkspace } from "./workspace/workspace-actions";
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
  const [settingsTab, setSettingsTab] = useState<
    "general" | "connections" | "providers" | "updates"
  >("general");
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
  } = useHerdr({
    savedSocket: saved?.socket || "",
    notify,
    setWorkspaces,
    connectionProfiles,
  });
  const skills = useSkills(sectionName, notify);
  const [tabMode, setTabMode] = useState(saved?.tabMode || false);
  const [sidebar, setSidebar] = useState(
    saved?.sidebar ?? window.innerWidth >= 760,
  );
  const [workspaceGrouping, setWorkspaceGrouping] = useState<
    "grouped" | "flat"
  >(saved?.workspaceGrouping || "grouped");
  // Agent and Chat render their lists into this slot of the shared sidebar.
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
    updateWorkspace,
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
  const addPanel = ws.addPanel;
  function switchWorkspace(id: string) {
    if (window.innerWidth < 760) setSidebar(false);
    ws.switchWorkspace(id);
  }
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const closeDialog = useCallback(() => setDialog(null), []);
  const target = resolveDialog(dialog, workspaces);
  const [workspaceQuery, setWorkspaceQuery] = useState("");
  const [compact, canvasRef] = useCompact();
  const [routines, setRoutines] = useState<Routine[]>(saved?.routines || []);
  const [fontScale, setFontScale] = useState(saved?.fontScale || 1);
  // Held here, not in SettingsDialog: the blocker must be active whenever the
  // app is open, not only while the preferences dialog happens to be mounted.
  const [keepAwake, setKeepAwake] = useKeepAwake();
  const connected = connection === "connected";
  const activeEndpoint = active.connection || socket;
  // Pane provenance (AC23-AC26, D5): only set when the active workspace is a
  // member of a merged row, and only in flat mode - the sidebar draws that
  // row, but the canvas is where a Herdr/agent pane actually names its host.
  const projectGit = useProjectGit(workspaces, active.id);
  const paneHostLabel = activeMergedHostLabel(
    workspaces,
    active,
    projectGit,
    connectionProfiles,
    workspaceGrouping,
  );
  /** Clicking a pane inside an expanded merged row (AC21) - unlike a plain
   * row's `showPanel`, the pane's owning workspace need not be active yet. */
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
    },
    notify,
  );

  function addExtensionPanel(extensionId: string, contributionId: string) {
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
      ws.insertPanel(panel);
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
  // The canvas owns the tab strip; App only needs the flag for the shell class.
  const useTabs = tabMode || compact || codePanels(active).length > 6;
  const blocked = blockedPanels(workspaces);

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
          panel ? showPanel(panel) : addPanel("files");
        }}
        tidy={() => {
          updateWorkspace(active.id, tidyWorkspace);
          setZoomed(null);
          showWorkspace();
          setTabMode(false);
        }}
        blocked={blocked}
        noticeCount={agentNotices.length}
      />
      <div className="app-body">
        {sidebar && (
          <Sidebar
            registry={extensionRegistry}
            openExtensionTarget={openExtensionTarget}
            runExtensionCommand={runExtensionCommand}
            mode={mode}
            onWorkspace={!section}
            currentRouteId={route.surfaceId}
            showWorkspace={showWorkspace}
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
            />
          ) : mode === "Agent" ? (
            <AgentsView slot={slot} />
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
            />
          ) : (
            <WorkspaceCanvas
              ws={ws}
              activeEndpoint={activeEndpoint}
              extensionRegistry={extensionRegistry}
              tabMode={tabMode}
              compact={compact}
              openPanelPicker={openPanelPicker}
              hostLabel={paneHostLabel}
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
          ) : dialog.kind === "sessions" ? (
            <SessionsDialog
              registry={extensionRegistry}
              openExtensionTarget={openExtensionTarget}
              cwd={active.cwd}
              connection={active.connection}
              workspaces={workspaces}
              activeId={active.id}
              onCloseSessions={endSessions}
              onShow={(w, p) => {
                switchWorkspace(w.id);
                setZoomed(p.id);
                closeDialog();
              }}
            />
          ) : dialog.kind === "close-session" && target?.panel ? (
            <CloseSessionDialog
              workspace={target!.workspace}
              panel={target!.panel!}
              onHide={() => {
                ws.hidePanel(target!.workspace.id, target!.panel!.id);
                closeDialog();
              }}
              onEnd={async () => {
                await endSessions([
                  { workspace: target!.workspace, panel: target!.panel! },
                ]);
                closeDialog();
              }}
            />
          ) : dialog.kind === "workspace-actions" && target ? (
            <ClaudeMcpSettings
              cwd={target!.workspace.cwd}
              endpoint={target!.workspace.connection}
              remote={target!.workspace.connection?.startsWith("ssh:") || false}
              workspaceName={target!.workspace.name}
              sessionCount={target!.workspace.panels.length}
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
                await addPanel(...args);
                closeDialog();
              }}
              addExtensionPanel={addExtensionPanel}
              extensionRegistry={extensionRegistry}
              connected={connected}
              hostContext={{
                workspaces,
                projectGit,
                connectionProfiles,
                workspaceGrouping,
              }}
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
              onCreate={async (...args) => {
                if (await ws.createWorkspace(...args)) closeDialog();
              }}
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
              blocked={blocked}
              openUpdates={() => setDialog({ kind: "updates" })}
              showBlockedPanel={({ workspace, panel }) => {
                switchWorkspace(workspace.id);
                setZoomed(panel.id);
                closeDialog();
              }}
            />
          ))}
      </DialogHost>
    </div>
  );
}
