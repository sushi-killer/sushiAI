import {
  Bell,
  Columns2,
  Download,
  FolderOpen,
  LayoutGrid,
  PanelLeft,
} from "lucide-react";
import { ExtensionActionSlot } from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { ExtensionRouteState } from "./navigation.ts";
import type { NavigationContribution } from "../extensions/types.ts";
import { extensionRoute, type CoreMode } from "../extensions/routes.ts";
import type { UpdateState, Workspace } from "../types";

export function TitleBar({
  nav,
  active,
  updates,
  tabMode,
  setTabMode,
  sidebar,
  setSidebar,
  setZoomed,
  openDialog,
  primaryExtensionNavigation,
  extensionRegistry,
  runExtensionCommand,
  tidy,
  openFiles,
  blocked,
  noticeCount,
}: {
  nav: {
    mode: CoreMode;
    /** A page is open, of either kind. Not its name: the toolbar behaves the
     * same whichever page it is. */
    section: boolean;
    /** The route the shell is on, so a contributed entry marks itself the same
     * way a core control does. The title bar never learns whose it is. */
    currentRouteId: string;
    setMode(mode: CoreMode): void;
    openExtension(target: ExtensionRouteState): void;
    showWorkspace(): void;
  };
  active: Workspace;
  updates: UpdateState | null;
  tabMode: boolean;
  setTabMode(value: boolean): void;
  sidebar: boolean;
  setSidebar(value: boolean): void;
  setZoomed(value: string | null): void;
  openDialog(name: "updates" | "notifications"): void;
  primaryExtensionNavigation: NavigationContribution[];
  extensionRegistry: ExtensionRegistry;
  runExtensionCommand(extensionId: string, commandId: string): void;
  tidy(): void;
  openFiles(): void;
  blocked: unknown[];
  noticeCount: number;
}) {
  const { mode, section, currentRouteId } = nav;
  const { setMode, openExtension, showWorkspace } = nav;
  // A contributed entry in this switch marks itself, so the mode button behind
  // it must let go - otherwise two buttons in one segmented control read as
  // selected. Computed from the route id: the title bar still learns nothing
  // about which extension it is.
  const contributedCurrent = primaryExtensionNavigation.some(
    (item) =>
      extensionRoute(item.extensionId, item.targetSurfaceId).surfaceId ===
      currentRouteId,
  );
  return (
    <header className="titlebar">
      <div className="titlebar-left">
        {!window.bridge && (
          <div className="traffic-lights">
            <i />
            <i />
            <i />
          </div>
        )}
        <div className="brand">
          <span className="brand-symbol">
            <img src="./sushi.svg" width="22" height="22" alt="sushiAI" />
          </span>
          <strong>sushiAI</strong>
        </div>
        <button
          className="icon-button sidebar-toggle"
          aria-label="Toggle sidebar"
          title="Toggle sidebar · ⌘B"
          onClick={() => setSidebar(!sidebar)}
        >
          <PanelLeft size={14} />
        </button>
      </div>
      <div className="mode-switch">
        {(["Agent", "Code", "Chat"] as const).map((item) => (
          <button
            key={item}
            className={mode === item && !contributedCurrent ? "active" : ""}
            onClick={() => {
              setMode(item);
              setZoomed(null);
            }}
          >
            {item}
          </button>
        ))}
        {primaryExtensionNavigation.map((item) => {
          return (
            <button
              key={`${item.extensionId}:${item.id}`}
              className={
                extensionRoute(item.extensionId, item.targetSurfaceId)
                  .surfaceId === currentRouteId
                  ? "active"
                  : ""
              }
              onClick={() => {
                openExtension({
                  extensionId: item.extensionId,
                  surfaceId: item.targetSurfaceId,
                });
                setZoomed(null);
              }}
            >
              {item.label}
            </button>
          );
        })}
      </div>
      <div className="titlebar-right">
        {(mode === "Code" || !!section) && (
          <>
            <ExtensionActionSlot
              registry={extensionRegistry}
              placement="workspace.toolbar.start"
              onRun={runExtensionCommand}
            />
            <button
              className={`icon-button ${tabMode ? "active" : ""}`}
              title={tabMode ? "Switch to split panels" : "Switch to tabs"}
              aria-pressed={tabMode}
              onClick={() => {
                setTabMode(!tabMode);
                setZoomed(null);
              }}
            >
              <Columns2 size={14} />
            </button>
            <button
              className="icon-button"
              title="Files and Git"
              onClick={openFiles}
            >
              <FolderOpen size={14} />
            </button>
            <ExtensionActionSlot
              registry={extensionRegistry}
              placement="workspace.toolbar.before-tidy"
              onRun={runExtensionCommand}
            />
            <button className="tidy" onClick={tidy} title="Arrange all panels">
              <LayoutGrid size={12} /> Tidy
            </button>
            <ExtensionActionSlot
              registry={extensionRegistry}
              placement="workspace.toolbar.after-tidy"
              onRun={runExtensionCommand}
            />
            <ExtensionActionSlot
              registry={extensionRegistry}
              placement="workspace.toolbar.end"
              onRun={runExtensionCommand}
            />
          </>
        )}
        {updates?.release && (
          <button
            className="update-indicator"
            aria-label="Software update available"
            onClick={() => openDialog("updates")}
          >
            <Download size={13} />
            <span>
              {updates.phase === "downloading"
                ? `${updates.progress}%`
                : updates.phase === "ready"
                  ? "Update ready"
                  : "Update available"}
            </span>
          </button>
        )}
        <button
          className="icon-button notification-button"
          aria-label="Notifications"
          onClick={() => openDialog("notifications")}
        >
          <Bell size={14} />
          {(blocked.length > 0 || updates?.release || noticeCount > 0) && <i />}
        </button>
      </div>
    </header>
  );
}
