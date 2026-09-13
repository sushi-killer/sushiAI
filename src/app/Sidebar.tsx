import {
  ChevronDown,
  CircleHelp,
  ChevronRight,
  LayoutDashboard,
  MoreHorizontal,
  Plug,
  Plus,
  Search,
  Settings,
  Sparkles,
  TerminalSquare,
  Workflow,
  X,
} from "lucide-react";
import { Icon } from "../PanelIcon.tsx";
import type React from "react";
import {
  ExtensionActionSlot,
  ExtensionIcon,
} from "../extensions/ExtensionSlots.tsx";
import {
  extensionRoute,
  navigationFor,
  routeFromLegacy,
} from "../extensions/routes.ts";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import { codePanels } from "../workspaceState.ts";
import type { Panel, Workspace } from "../types";

/** The left nav is the app's own sections followed by whatever manifests add.
 * A manifest orders its entries among themselves and cannot reach above a
 * section of the app. Core reads `order` and nothing else - it never learns
 * which extension an entry came from. */
function primaryNav({
  currentRouteId,
  workspaces,
  registry,
  openDialog,
  showWorkspace,
  toggleCoreSection,
  openExtensionTarget,
}: {
  /** The route the shell is on. Every entry marks itself by comparing its own
   * route id with this, so a contributed page is highlighted by the same rule
   * as Skills and core never learns which extension is open. */
  currentRouteId: string;
  workspaces: Workspace[];
  registry: ExtensionRegistry;
  openDialog(name: "sessions" | "workspace"): void;
  showWorkspace(): void;
  toggleCoreSection(section: string): void;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
}) {
  const builtin = [
    { label: "Dashboard", Glyph: LayoutDashboard, count: workspaces.length },
    { label: "Sessions", Glyph: TerminalSquare },
    { label: "Routines", Glyph: Workflow },
    { label: "Extensions", Glyph: Plug },
    { label: "Skills", Glyph: Sparkles },
  ].map(({ label, Glyph, count }) => ({
    key: label,
    label,
    count,
    icon: <Glyph size={14} />,
    current: routeFromLegacy("Code", label).surfaceId === currentRouteId,
    open: () => {
      if (label !== "Sessions") return toggleCoreSection(label);
      showWorkspace();
      openDialog("sessions");
    },
  }));
  const contributed = navigationFor(registry, "sidebar.primary").map(
    (item) => ({
      key: `${item.extensionId}:${item.id}`,
      label: item.label,
      count: undefined,
      icon: <ExtensionIcon icon={item.icon} />,
      current:
        extensionRoute(item.extensionId, item.targetSurfaceId).surfaceId ===
        currentRouteId,
      open: () => openExtensionTarget(item.extensionId, item.targetSurfaceId),
    }),
  );
  // Appended, not merged and re-sorted: a section of the app always comes
  // before anything a manifest adds, whatever order it asks for, and the
  // entries it adds keep the one order the registry hands every placement.
  return [...builtin, ...contributed];
}

function ProfileBlock({
  notify,
  openSettings,
}: {
  notify(text: string): void;
  openSettings(): void;
}) {
  return (
    <div className="profile">
      <span className="avatar">
        <img src="./sushi.svg" width="22" height="22" alt="" />
      </span>
      <div>
        <strong>sushiAI</strong>
        <span>ON YOUR MAC</span>
      </div>
      <button
        className="icon-button"
        title="Keyboard shortcuts"
        onClick={() =>
          notify(
            "⌘K Add panel · ⌘B Toggle sidebar · ⌘Enter Focus panel · Esc Restore layout. Drag panel headers to rearrange; drag dividers to resize.",
          )
        }
      >
        <CircleHelp size={13} />
      </button>
      <button
        className="icon-button"
        aria-label="Settings"
        onClick={() => openSettings()}
      >
        <Settings size={13} />
      </button>
    </div>
  );
}

export function Sidebar({
  mode,
  onWorkspace,
  currentRouteId,
  showWorkspace,
  toggleCoreSection,
  openDialog,
  manageWorkspace,
  workspaces,
  active,
  workspaceQuery,
  setWorkspaceQuery,
  switchWorkspace,
  showPanel,
  requestClose,
  setSlot,
  selected,
  connected,
  connection,
  openSettings,
  totalPanels,
  notify,
  registry,
  openExtensionTarget,
  runExtensionCommand,
}: {
  mode: string;
  /** No page is open, so the canvas is showing this workspace's panels. */
  onWorkspace: boolean;
  currentRouteId: string;
  showWorkspace(): void;
  toggleCoreSection(section: string): void;
  openDialog(name: "sessions" | "workspace"): void;
  manageWorkspace(workspace: Workspace): void;
  workspaces: Workspace[];
  active: Workspace;
  workspaceQuery: string;
  setWorkspaceQuery(value: string): void;
  switchWorkspace(id: string): void;
  showPanel(panel: Panel): void;
  requestClose(value: { workspace: Workspace; panel?: Panel }): void;
  setSlot(element: HTMLElement | null): void;
  selected: string;
  connected: boolean;
  connection: string;
  openSettings(): void;
  totalPanels: number;
  notify(text: string): void;
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  runExtensionCommand(extensionId: string, commandId: string): void;
}) {
  return (
    <aside className="sidebar">
      {/* The sidebar is one container; Code fills it with nav, workspaces
                and footer, Agent and Chat load only their list into it. */}
      {mode === "Code" ? (
        <>
          <nav className="primary-nav">
            {primaryNav({
              currentRouteId,
              workspaces,
              registry,
              openDialog,
              showWorkspace,
              toggleCoreSection,
              openExtensionTarget,
            }).map((item) => (
              <button
                key={item.key}
                className={item.current ? "nav-item current" : "nav-item"}
                aria-current={item.current ? "page" : undefined}
                onClick={item.open}
              >
                {item.icon}
                <span>{item.label}</span>
                {item.count !== undefined && (
                  <span className="count">{item.count}</span>
                )}
              </button>
            ))}
          </nav>
          <div className="workspace-section">
            <div className="section-label">
              <span>Workspaces</span>
              <button
                className="icon-button"
                aria-label="New workspace"
                onClick={() => openDialog("workspace")}
              >
                <Plus size={14} />
              </button>
            </div>
            <label className="workspace-search">
              <Search size={12} />
              <input
                aria-label="Search workspaces"
                placeholder="Find workspace…"
                value={workspaceQuery}
                onChange={(e) => setWorkspaceQuery(e.target.value)}
              />
            </label>
            <div className="workspace-list">
              {workspaces
                .filter((w) =>
                  w.name.toLowerCase().includes(workspaceQuery.toLowerCase()),
                )
                .map((w) => (
                  <div key={w.id} className="workspace-item">
                    <button
                      className="workspace-more"
                      title={`Manage workspace ${w.name}`}
                      onClick={() => manageWorkspace(w)}
                    >
                      <MoreHorizontal size={13} />
                    </button>
                    <ExtensionActionSlot
                      registry={registry}
                      placement="workspace.folder.actions"
                      className="workspace-more extension-action"
                      onRun={runExtensionCommand}
                    />
                    <button
                      className={`workspace-name ${w.id === active.id && onWorkspace ? "active" : ""}`}
                      onClick={() => switchWorkspace(w.id)}
                      title={w.cwd}
                    >
                      {w.id === active.id ? (
                        <ChevronDown size={12} />
                      ) : (
                        <ChevronRight size={12} />
                      )}
                      <span>{w.name}</span>
                      {w.herdrId && (
                        <i
                          className={`status-dot ${connected ? "green" : ""}`}
                          title="Herdr workspace"
                        />
                      )}
                    </button>
                    {w.id === active.id && onWorkspace && (
                      <div className="workspace-panels">
                        {codePanels(w).map((p) => (
                          <button
                            key={p.id}
                            className={selected === p.id ? "selected" : ""}
                            onClick={() => showPanel(p)}
                            title={p.title}
                          >
                            <Icon kind={p.kind} agent={p.agent} />
                            <span>
                              {p.kind === "browser" && p.url
                                ? p.url
                                    .replace(/^https?:\/\//, "")
                                    .replace(/\/$/, "")
                                : p.title}
                            </span>
                            {p.status === "working" && (
                              <i className="status-dot green pulse" />
                            )}
                            {p.status === "blocked" && (
                              <i className="status-dot yellow" />
                            )}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
            </div>
          </div>
          <footer className="sidebar-footer">
            <button className="backend-status" onClick={() => openSettings()}>
              <span>Herdr</span>
              <span className={`status-pill ${connected ? "live" : ""}`}>
                <i />
                {connected
                  ? "Connected"
                  : connection === "connecting"
                    ? "Connecting"
                    : "Offline"}
              </span>
            </button>
            <div className="session-count">
              <span>Panels</span>
              <span className="count">{totalPanels}</span>
            </div>
            <ProfileBlock notify={notify} openSettings={openSettings} />
          </footer>
        </>
      ) : (
        <>
          <div className="sidebar-slot" ref={setSlot} />
          <footer className="sidebar-footer">
            <ProfileBlock notify={notify} openSettings={openSettings} />
          </footer>
        </>
      )}
    </aside>
  );
}
