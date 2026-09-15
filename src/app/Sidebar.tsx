import { useState } from "react";
import {
  ChevronDown,
  CircleHelp,
  ChevronRight,
  FolderTree,
  Globe,
  LayoutDashboard,
  LayoutList,
  MoreHorizontal,
  Plug,
  Plus,
  Search,
  Server,
  Settings,
  Sparkles,
  TerminalSquare,
  Workflow,
} from "lucide-react";
import { Icon } from "../PanelIcon.tsx";
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
import {
  LOCAL_GROUP,
  computeMergeGroups,
  groupKey,
  groupLabel,
  groupStatus,
  isHidden,
  mergedMarkerAccessibleName,
  memberLabel,
  mergedRowStatusKey,
  mixedRemotes,
  shouldCollapseHostMarkers,
  type MergeGroup,
} from "./workspaceMerge.ts";
import type { ConnectionProfile, Panel, Workspace } from "../types";
import type { ProjectGit } from "./useProjectGit.ts";

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
  selectHostPane,
  setSlot,
  selected,
  connected,
  connection,
  localSocket,
  connectionProfiles,
  statusByEndpoint,
  projectGit,
  workspaceGrouping,
  setWorkspaceGrouping,
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
  /** Clicking a pane inside an expanded merged row (AC21): unlike `showPanel`,
   * its owning workspace need not be the active one yet. */
  selectHostPane(workspace: Workspace, panel: Panel): void;
  requestClose(value: { workspace: Workspace; panel?: Panel }): void;
  setSlot(element: HTMLElement | null): void;
  selected: string;
  connected: boolean;
  connection: string;
  /** This Mac's own Herdr socket, so the "Local" group can look up its
   * real poll status the same way an SSH group looks up its own. */
  localSocket: string;
  connectionProfiles: ConnectionProfile[];
  /** Real, current poll status per endpoint - every connected host is polled
   * independently, so this is never just the default connection's status. */
  statusByEndpoint: Record<string, string>;
  /** Each workspace's git identity - remote, shared git dir, checkout and
   * branch - which decides when two workspaces are one project. */
  projectGit: Record<string, ProjectGit>;
  /** "grouped" sections workspaces under a collapsible host header; "flat"
   * is a single list with a small tag naming a remote workspace's host. */
  workspaceGrouping: "grouped" | "flat";
  setWorkspaceGrouping(value: "grouped" | "flat"): void;
  openSettings(): void;
  totalPanels: number;
  notify(text: string): void;
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  runExtensionCommand(extensionId: string, commandId: string): void;
}) {
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(
    () => new Set(),
  );
  const toggleGroup = (key: string) =>
    setCollapsedGroups((current) => {
      const next = new Set(current);
      next.has(key) ? next.delete(key) : next.add(key);
      return next;
    });
  /** UX5: which merged rows the user opened by hand, kept for the app
   * session only - it survives the grouping toggle and search (this state
   * does not depend on either) and resets on restart, same as every other
   * piece of Sidebar's own local state. */
  const [expandedMergedGroups, setExpandedMergedGroups] = useState<Set<string>>(
    () => new Set(),
  );
  const toggleMergedGroup = (id: string) =>
    setExpandedMergedGroups((current) => {
      const next = new Set(current);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  /** One workspace row - each carries a `tag` (the owning host's label) next
   * to its name when it isn't this Mac's, or its branch when another row
   * shares its name, since the list is always flat, and
   * a small `mixed` marker when this project also runs on the other kind of
   * machine (a plain signal, not a merge - every copy stays its own row). */
  const renderRow = (
    w: Workspace,
    live: boolean,
    tag?: string,
    mixed?: boolean,
  ) => (
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
        {mixed && (
          <span
            className="host-mix"
            title="This project also runs on the other kind of machine"
          >
            <Server size={10} />
            <Globe size={10} />
          </span>
        )}
        {tag && (
          <span className="remote-tag">
            {groupKey(w.connection) !== LOCAL_GROUP && <Globe size={10} />}
            {tag}
          </span>
        )}
        {w.herdrId && (
          <i
            className={`status-dot ${live ? "green" : ""}`}
            title={tag ? `${tag} workspace` : "Herdr workspace"}
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
                  ? p.url.replace(/^https?:\/\//, "").replace(/\/$/, "")
                  : p.title}
              </span>
              {p.status === "working" && (
                <i className="status-dot green pulse" />
              )}
              {p.status === "blocked" && <i className="status-dot yellow" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
  /** A row standing for two or more workspaces that share a project (D4).
   * Clicking the name never changes host (D3, AC15) - it switches to the
   * active member when there is one (AC17, exactly as an unmerged row would)
   * or toggles the combined session list otherwise (AC16). The list under it
   * concatenates every member's own panes, host by host (AC19); clicking one
   * of those is the only thing here that changes the active workspace and
   * endpoint (AC21). */
  const renderMergedRow = (group: MergeGroup) => {
    const activeMember = group.members.find(
      (m) => m.workspace.id === active.id,
    );
    const isActiveGroup = Boolean(activeMember) && onWorkspace;
    const expanded = isActiveGroup || expandedMergedGroups.has(group.id);
    const anchor =
      group.members.find((m) => m.hostKey === LOCAL_GROUP) || group.members[0];
    const statusKey = mergedRowStatusKey(group, active.id);
    const live =
      groupStatus(statusKey, localSocket, statusByEndpoint) === "connected";
    const collapse = shouldCollapseHostMarkers(group, connectionProfiles);
    // On one machine every pane's icon would be identical noise.
    const manyHosts = new Set(group.members.map((m) => m.hostKey)).size > 1;
    const markerName = mergedMarkerAccessibleName(
      group,
      connectionProfiles,
      localSocket,
      statusByEndpoint,
    );
    const rowTitle = group.members
      .map(
        (m) =>
          `${memberLabel(group, m, connectionProfiles)} - ${m.workspace.cwd}`,
      )
      .join("\n");
    return (
      <div key={group.id} className="workspace-item">
        {/* Manage acts on the host you are on; open a pane on the other host
            to manage that one. */}
        <button
          className="workspace-more"
          title={`Manage workspace ${anchor.workspace.name}`}
          onClick={() => manageWorkspace((activeMember || anchor).workspace)}
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
          className={`workspace-name ${isActiveGroup ? "active" : ""}`}
          onClick={() =>
            activeMember
              ? switchWorkspace(activeMember.workspace.id)
              : toggleMergedGroup(group.id)
          }
          title={rowTitle}
        >
          {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          <span>{anchor.workspace.name}</span>
          <span
            className="host-marker-group"
            role="img"
            aria-label={markerName}
            title={markerName}
          >
            {collapse ? (
              <span className="remote-tag">
                {group.members.length} {group.worktrees ? "worktrees" : "hosts"}
              </span>
            ) : (
              group.members.map((m) => {
                const MarkerIcon = m.hostKey === LOCAL_GROUP ? Server : Globe;
                return (
                  <span className="remote-tag" key={m.workspace.id}>
                    {(!group.worktrees || m.hostKey !== LOCAL_GROUP) && (
                      <MarkerIcon size={10} />
                    )}
                    {memberLabel(group, m, connectionProfiles)}
                  </span>
                );
              })
            )}
          </span>
          <i
            className={`status-dot ${live ? "green" : ""}`}
            title={`Status for ${groupLabel(statusKey, connectionProfiles)}.`}
          />
        </button>
        {expanded && (
          <div className="workspace-panels">
            {group.members.flatMap((m) => {
              const label = memberLabel(group, m, connectionProfiles);
              const offline =
                groupStatus(m.hostKey, localSocket, statusByEndpoint) ===
                "offline";
              const HostIcon = m.hostKey === LOCAL_GROUP ? Server : Globe;
              return codePanels(m.workspace).map((p) => (
                <button
                  key={`${m.workspace.id}:${p.id}`}
                  className={`${selected === p.id && m.workspace.id === active.id ? "selected" : ""} ${offline ? "offline" : ""}`}
                  onClick={() => selectHostPane(m.workspace, p)}
                  title={`${p.title} - ${label}${offline ? " (offline)" : ""}`}
                >
                  <Icon kind={p.kind} agent={p.agent} />
                  <span>
                    {p.kind === "browser" && p.url
                      ? p.url.replace(/^https?:\/\//, "").replace(/\/$/, "")
                      : p.title}
                  </span>
                  {group.worktrees && (
                    <span className="remote-tag pane-branch">{label}</span>
                  )}
                  {manyHosts && (
                    <HostIcon
                      size={10}
                      className="pane-host-icon"
                      aria-label={label}
                    />
                  )}
                  {p.status === "working" && (
                    <i className="status-dot green pulse" />
                  )}
                  {p.status === "blocked" && (
                    <i className="status-dot yellow" />
                  )}
                </button>
              ));
            })}
          </div>
        )}
      </div>
    );
  };
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
              <button
                type="button"
                className="workspace-grouping-toggle"
                aria-label={
                  workspaceGrouping === "grouped"
                    ? "Show as a flat list"
                    : "Group by host"
                }
                title={
                  workspaceGrouping === "grouped"
                    ? "Show as a flat list"
                    : "Group by host"
                }
                onClick={() =>
                  setWorkspaceGrouping(
                    workspaceGrouping === "grouped" ? "flat" : "grouped",
                  )
                }
              >
                {workspaceGrouping === "grouped" ? (
                  <LayoutList size={12} />
                ) : (
                  <FolderTree size={12} />
                )}
              </button>
            </label>
            <div className="workspace-list">
              {(() => {
                const visible = workspaces.filter(
                  (w) =>
                    w.name
                      .toLowerCase()
                      .includes(workspaceQuery.toLowerCase()) &&
                    !isHidden(w.connection, connectionProfiles),
                );
                const mixed = mixedRemotes(visible, projectGit);
                const filtering = workspaceQuery.trim().length > 0;
                if (workspaceGrouping === "flat") {
                  // A search that matches nothing used to render an empty
                  // array, i.e. a blank void with no explanation.
                  if (!visible.length)
                    return (
                      <p className="workspace-group-empty">
                        {filtering
                          ? `No workspaces match "${workspaceQuery}".`
                          : "No open workspaces yet."}
                      </p>
                    );
                  // UX1: the list never waits on remotes and never reshuffles
                  // once they arrive - a merged row takes the position of its
                  // topmost member in this same order, and merging only ever
                  // removes the rows below it (`consumed`), never inserts or
                  // reorders one.
                  const mergeGroups = computeMergeGroups(
                    visible,
                    projectGit,
                    connectionProfiles,
                  );
                  const consumed = new Set<string>();
                  const nameCount = new Map<string, number>();
                  for (const w of visible)
                    nameCount.set(w.name, (nameCount.get(w.name) ?? 0) + 1);
                  return visible.map((w) => {
                    if (consumed.has(w.id)) return null;
                    const group = mergeGroups.get(w.id);
                    if (group) {
                      for (const member of group.members)
                        consumed.add(member.workspace.id);
                      return renderMergedRow(group);
                    }
                    const key = groupKey(w.connection);
                    const live =
                      groupStatus(key, localSocket, statusByEndpoint) ===
                      "connected";
                    const tag =
                      key !== LOCAL_GROUP
                        ? groupLabel(key, connectionProfiles)
                        : (nameCount.get(w.name) ?? 0) > 1
                          ? projectGit[w.id]?.branch || undefined
                          : undefined;
                    return renderRow(
                      w,
                      live,
                      tag,
                      mixed.has(projectGit[w.id]?.remote ?? ""),
                    );
                  });
                }
                // Every group a connected, visible host owns shows up here
                // even with zero workspaces right now - a live, empty
                // connection is still worth seeing next to This Mac, not
                // silently absent.
                const groups = new Map<string, Workspace[]>();
                groups.set(LOCAL_GROUP, []);
                for (const profile of connectionProfiles)
                  if (profile.connected && !profile.hidden)
                    groups.set(`ssh:${profile.id}`, []);
                for (const w of visible) {
                  const key = groupKey(w.connection);
                  (groups.get(key) || groups.set(key, []).get(key)!).push(w);
                }
                const keys = [...groups.keys()].sort((a, b) =>
                  a === LOCAL_GROUP
                    ? -1
                    : b === LOCAL_GROUP
                      ? 1
                      : groupLabel(a, connectionProfiles).localeCompare(
                          groupLabel(b, connectionProfiles),
                        ),
                );
                return keys.map((key) => {
                  const members = groups.get(key)!;
                  const label = groupLabel(key, connectionProfiles);
                  const status = groupStatus(
                    key,
                    localSocket,
                    statusByEndpoint,
                  );
                  const live = status === "connected";
                  const collapsed = collapsedGroups.has(key);
                  return (
                    <div className="workspace-group" key={key}>
                      <button
                        className="workspace-group-header"
                        onClick={() => toggleGroup(key)}
                        title={`${members.length} workspace${members.length === 1 ? "" : "s"}`}
                      >
                        {collapsed ? (
                          <ChevronRight size={11} />
                        ) : (
                          <ChevronDown size={11} />
                        )}
                        {key === LOCAL_GROUP ? (
                          <Server size={12} />
                        ) : (
                          <Globe size={12} />
                        )}
                        <span>{label}</span>
                        <span className={`status-pill ${live ? "live" : ""}`}>
                          <i />
                          {live
                            ? "Connected"
                            : status === "connecting"
                              ? "Connecting"
                              : "Offline"}
                        </span>
                        <span className="count">{members.length}</span>
                      </button>
                      {!collapsed && (
                        <div className="workspace-group-items">
                          {members.length === 0 && (
                            <p className="workspace-group-empty">
                              {/* Branching on `live` alone claimed the host
                                  had no workspaces while a filter was simply
                                  hiding them - the count next to Dashboard
                                  still read the real total. */}
                              {filtering
                                ? "None match your search."
                                : live
                                  ? "No open workspaces on this host yet."
                                  : "Not connected."}
                            </p>
                          )}
                          {members.map((w) =>
                            renderRow(
                              w,
                              live,
                              undefined,
                              mixed.has(projectGit[w.id]?.remote ?? ""),
                            ),
                          )}
                        </div>
                      )}
                    </div>
                  );
                });
              })()}
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
