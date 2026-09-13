import { useEffect, useRef, useState } from "react";
import { Columns2, Plus, X } from "lucide-react";
import { resize } from "../layout.ts";
import { codePanels } from "../workspaceState.ts";
import { Empty } from "../app/Empty.tsx";
import { LayoutView, PanelHost } from "../WorkspacePanels.tsx";
import { Icon } from "../PanelIcon.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { WorkspaceController } from "./useWorkspaces.ts";

/** The panel surface: a zoomed panel, the tab strip, the split layout or the
 * empty state. Tab history and the compact breakpoint are its own business. */
export function WorkspaceCanvas({
  ws,
  activeEndpoint,
  extensionRegistry,
  tabMode,
  compact,
  openPanelPicker,
}: {
  ws: WorkspaceController;
  activeEndpoint: string;
  extensionRegistry: ExtensionRegistry;
  tabMode: boolean;
  compact: boolean;
  openPanelPicker(): void;
}) {
  const {
    active,
    selected,
    zoomed,
    dragId,
    setSelected,
    setZoomed,
    focusPanel,
    startPanelDrag,
    endPanelDrag,
    zoomPanel,
    startPanel,
    navigatePanel,
    setPanelAgent,
    cancelPanelChat,
    renamePanel,
    closePanel,
    drop,
    sendChat,
    openHTML,
    updateWorkspace,
  } = ws;
  const visitedTabs = useRef(new Set<string>());
  const filteredPanels = codePanels(active);
  const compactId =
    filteredPanels.find((p) => p.id === selected)?.id || filteredPanels[0]?.id;
  const useTabs = tabMode || compact || filteredPanels.length > 6;
  if (compactId) visitedTabs.current.add(compactId);
  const visibleLayout = active.layout;
  function renderPanel(id: string) {
    const panel = active.panels.find((p) => p.id === id);
    if (!panel) return null;
    return (
      <PanelHost
        key={panel.id}
        panel={panel}
        cwd={panel.filesTarget?.root || active.cwd}
        socket={activeEndpoint}
        endpoint={active.connection}
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
        onRename={renamePanel}
        onStart={startPanel}
        onNavigate={navigatePanel}
        onHTML={openHTML}
        onSend={sendChat}
        onCancel={cancelPanelChat}
        onAgent={setPanelAgent}
        extensionRegistry={extensionRegistry}
      />
    );
  }
  return (
    <>
      {zoomed && active.panels.some((p) => p.id === zoomed) ? (
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
          onResize={(id, ratio) =>
            updateWorkspace(active.id, (w) => ({
              ...w,
              layout: w.layout ? resize(w.layout, id, ratio) : null,
            }))
          }
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
