import { useCallback, useMemo, useRef } from "react";
import { Columns2, Plus, X } from "lucide-react";
import { findPanelOwner } from "./workspace-actions.ts";
import { codePanels } from "../workspaceState.ts";
import { liveWorktreeBranches } from "./worktreeSessions.ts";
import { Empty } from "../app/Empty.tsx";
import { LayoutView, PanelHost } from "../WorkspacePanels.tsx";
import { Icon } from "../PanelIcon.tsx";
import type {
  LaunchAgentRequest,
  WorktreeSessions,
} from "../extensions/coreViews.ts";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { WorkspaceController } from "./useWorkspaces.ts";
import type { MergedCanvas } from "./mergedLayouts.ts";

/** The panel surface: a zoomed panel, the tab strip, the split layout or the
 * empty state. Tab history and the compact breakpoint are its own business. */
export function WorkspaceCanvas({
  ws,
  activeEndpoint,
  extensionRegistry,
  tabMode,
  compact,
  openPanelPicker,
  openConnections,
  merged,
}: {
  ws: WorkspaceController;
  activeEndpoint: string;
  extensionRegistry: ExtensionRegistry;
  tabMode: boolean;
  compact: boolean;
  openPanelPicker(): void;
  openConnections(): void;
  /** The active workspace's merge group (flat mode only, from
   * useMergedCanvas). When set (C1), every member's code panels are drawn
   * together instead of just the active workspace's own. */
  merged: MergedCanvas;
}) {
  const {
    active,
    selected,
    zoomed,
    dragId,
    setSelected,
    focusPanel,
    startPanelDrag,
    endPanelDrag,
    zoomPanel,
    startPanel,
    reopenPanel,
    updatePanel,
    navigatePanel,
    setPanelAgent,
    cancelPanelChat,
    renamePanel,
    closePanel,
    drop,
    sendChat,
    openHTML,
    resizeSplit,
    patchCompanion,
  } = ws;
  // Both read the controller through a ref, so their identity does not change
  // with every render and the memoised PanelHost props stay stable.
  const wsRef = useRef(ws);
  wsRef.current = ws;
  // The agent starts in the workspace that owns the pane with the companion,
  // which in a merged view is not necessarily the active one.
  const launchAgent = useCallback(
    (panelId: string, request: LaunchAgentRequest) =>
      wsRef.current.addPanel(
        "agent",
        request.agent,
        undefined,
        undefined,
        undefined,
        "herdr",
        findPanelOwner(wsRef.current.workspaces, panelId)?.id,
        { branch: request.branch, base: request.base },
        undefined,
        undefined,
        request.prompt,
      ),
    [],
  );
  const worktrees = useMemo<WorktreeSessions>(
    () => ({
      branches: liveWorktreeBranches(ws.workspaces),
      open: (branch) => {
        const target = wsRef.current.workspaces.find(
          (w) => w.worktreeBranch === branch && w.panels.some((p) => !p.ended),
        );
        if (target) wsRef.current.selectWorkspace(target.id);
      },
    }),
    [ws.workspaces],
  );
  const visitedTabs = useRef(new Set<string>());
  const group = merged.group;
  const paneById = new Map(merged.panes.map((mp) => [mp.panel.id, mp]));
  const filteredPanels = group
    ? merged.panes.map((mp) => mp.panel)
    : codePanels(active);
  const compactId =
    filteredPanels.find((p) => p.id === selected)?.id || filteredPanels[0]?.id;
  const useTabs = tabMode || compact || filteredPanels.length > 6;
  if (compactId) visitedTabs.current.add(compactId);
  const visibleLayout = group ? merged.layout : active.layout;
  function renderPanel(id: string) {
    const own = paneById.get(id);
    const panel =
      own?.panel || (!group && active.panels.find((p) => p.id === id));
    if (!panel) return null;
    return (
      <PanelHost
        key={panel.id}
        panel={panel}
        cwd={own ? own.cwd : panel.filesTarget?.root || active.cwd}
        socket={own ? own.socket : activeEndpoint}
        endpoint={own ? own.endpoint : active.connection}
        hostLabel={own?.hostLabel}
        selected={selected === id}
        zoomed={zoomed === id}
        dragging={!!dragId}
        onFocus={focusPanel}
        onDrag={startPanelDrag}
        onDragEnd={endPanelDrag}
        onDrop={drop}
        onClose={closePanel}
        onZoom={zoomPanel}
        onAdd={openPanelPicker}
        onOpenConnections={openConnections}
        onRename={renamePanel}
        onStart={startPanel}
        onReopen={reopenPanel}
        onPatch={updatePanel}
        onNavigate={navigatePanel}
        onHTML={openHTML}
        onSend={sendChat}
        onCancel={cancelPanelChat}
        onAgent={setPanelAgent}
        extensionRegistry={extensionRegistry}
        onLaunchAgent={launchAgent}
        worktrees={worktrees}
        onCompanion={patchCompanion}
      />
    );
  }
  return (
    <>
      {zoomed && filteredPanels.some((p) => p.id === zoomed) ? (
        renderPanel(zoomed)
      ) : useTabs && compactId ? (
        <div className="adaptive-workspace">
          <div
            className="panel-tabs"
            role="tablist"
            aria-label="Workspace panels"
          >
            {filteredPanels.map((p) => (
              <div
                className={`panel-tab ${p.id === compactId ? "active" : ""}`}
                key={p.id}
              >
                <button
                  role="tab"
                  aria-selected={p.id === compactId}
                  tabIndex={p.id === compactId ? 0 : -1}
                  onKeyDown={(event) => {
                    if (
                      ["ArrowLeft", "ArrowRight", "Home", "End"].includes(
                        event.key,
                      )
                    ) {
                      event.preventDefault();
                      const index = filteredPanels.findIndex(
                        (item) => item.id === p.id,
                      );
                      const next =
                        event.key === "Home"
                          ? 0
                          : event.key === "End"
                            ? filteredPanels.length - 1
                            : (index +
                                (event.key === "ArrowRight" ? 1 : -1) +
                                filteredPanels.length) %
                              filteredPanels.length;
                      setSelected(filteredPanels[next].id);
                      const tabs = event.currentTarget
                        .closest('[role="tablist"]')
                        ?.querySelectorAll<HTMLElement>('[role="tab"]');
                      tabs?.[next].focus();
                    }
                  }}
                  onClick={() => setSelected(p.id)}
                >
                  <Icon kind={p.kind} agent={p.agent} />
                  <span>{p.title}</span>
                </button>
                <button
                  className="tab-close"
                  aria-label={`Close tab ${p.title}`}
                  onClick={() => closePanel(p.id)}
                >
                  <X size={11} />
                </button>
              </div>
            ))}
            <button title="Add panel" onClick={openPanelPicker}>
              <Plus size={13} />
            </button>
          </div>
          <div className="adaptive-panel">
            {filteredPanels
              .filter((p) => visitedTabs.current.has(p.id))
              .map((p) => (
                <div
                  className="tab-panel"
                  key={p.id}
                  hidden={p.id !== compactId}
                >
                  {renderPanel(p.id)}
                </div>
              ))}
          </div>
        </div>
      ) : visibleLayout ? (
        <LayoutView
          layout={visibleLayout}
          renderPanel={renderPanel}
          onResize={resizeSplit}
        />
      ) : (
        <Empty
          icon={<Columns2 size={30} />}
          title="Space for your next idea."
          text="Add a panel to get started."
          action="Add panel"
          onAction={openPanelPicker}
        />
      )}
    </>
  );
}
