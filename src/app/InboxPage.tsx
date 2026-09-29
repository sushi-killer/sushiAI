import { useMemo, useState } from "react";
import {
  Check,
  Globe,
  Inbox as InboxIcon,
  ListChecks,
  Search,
  Server,
  Trash2,
} from "lucide-react";
import { Icon } from "../PanelIcon.tsx";
import {
  ExtensionNavSlot,
  ExtensionSectionSlot,
} from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import { PageFrame } from "./PageFrame.tsx";
import { Empty } from "./Empty.tsx";
import {
  cleanupSelection,
  type InboxGroup,
  type InboxRow,
} from "./attention.ts";
import { LOCAL_GROUP, groupKey, groupLabel } from "./workspaceMerge.ts";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import { ownerInboxRows } from "../orchestrator/ownerAttention.ts";
import type { TaskTarget } from "../orchestrator/notices.ts";
import type { Task } from "../orchestrator/types.ts";
import type { ConnectionProfile, Workspace } from "../types";

/** Rounded to whatever unit reads best - minutes under an hour, then hours,
 * then days. Approximate on purpose: this is "12m ago", not a stopwatch. */
function formatElapsed(ms: number): string {
  const minutes = Math.max(0, Math.round(ms / 60000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.round(hours / 24)}d`;
}

function matchesQuery(row: InboxRow, query: string): boolean {
  if (!query) return true;
  const needle = query.toLowerCase();
  return (
    row.panel.title.toLowerCase().includes(needle) ||
    row.workspace.name.toLowerCase().includes(needle)
  );
}

/** The attention queue: agents that need input or finished, plus everything
 * else still running or sitting idle, across every host. Sessions used to be
 * a manual list you opened to check on things; this is the same panels, but
 * it tells you which of them actually want you. */
export function InboxPage({
  groups,
  markSeen,
  ownerTasks,
  openOrchestratorTask,
  switchWorkspace,
  ws,
  connectionProfiles,
  registry,
  openExtensionTarget,
  cwd,
  connection,
  workspaces,
}: {
  groups: InboxGroup[];
  markSeen(panelId: string): void;
  ownerTasks: Task[];
  openOrchestratorTask(target: TaskTarget): void;
  switchWorkspace(id: string): void;
  ws: WorkspaceController;
  connectionProfiles: ConnectionProfile[];
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  cwd: string;
  connection?: string;
  workspaces: Workspace[];
}) {
  const [query, setQuery] = useState(""),
    [hostFilter, setHostFilter] = useState(""),
    [checked, setChecked] = useState<string[]>([]),
    [confirming, setConfirming] = useState(false),
    [busy, setBusy] = useState(false);
  const allRows = useMemo(
    () => groups.flatMap((group) => group.rows),
    [groups],
  );
  const hosts = useMemo(() => {
    const seen = new Map<string, string>();
    for (const row of allRows) {
      const key = groupKey(row.workspace.connection);
      if (!seen.has(key)) seen.set(key, groupLabel(key, connectionProfiles));
    }
    return [...seen.entries()].sort(([a], [b]) =>
      a === LOCAL_GROUP ? -1 : b === LOCAL_GROUP ? 1 : a.localeCompare(b),
    );
  }, [allRows, connectionProfiles]);
  // One host in the list means a host mark on every row says nothing.
  const manyHosts = hosts.length > 1;
  const visibleGroups = groups.map((group) => ({
    ...group,
    rows: group.rows.filter(
      (row) =>
        matchesQuery(row, query) &&
        (!hostFilter || groupKey(row.workspace.connection) === hostFilter),
    ),
  }));
  const visibleRows = visibleGroups.flatMap((group) => group.rows);
  const selected = visibleRows.filter((row) => checked.includes(row.panel.id));
  const visibleTasks = ownerInboxRows(ownerTasks, query, openOrchestratorTask);
  const empty = allRows.length === 0 && ownerTasks.length === 0;

  function toggle(panelId: string, on: boolean) {
    setChecked((ids) =>
      on ? [...ids, panelId] : ids.filter((id) => id !== panelId),
    );
    setConfirming(false);
  }
  function jump(row: InboxRow) {
    switchWorkspace(row.workspace.id);
    ws.setSelected(row.panel.id);
    ws.setZoomed(row.panel.id);
  }

  return (
    <PageFrame
      eyebrow="YOUR WORKSPACE"
      title="Inbox"
      description="Agents that need you, across every host."
      actions={
        <ExtensionNavSlot
          registry={registry}
          placement="sessions.navigation"
          className="secondary extension-nav"
          onOpen={openExtensionTarget}
        />
      }
    >
      <ExtensionSectionSlot
        registry={registry}
        host="sessions.section"
        cwd={cwd}
        connection={connection}
        workspaces={workspaces}
      />
      {empty ? (
        <Empty
          icon={<InboxIcon size={28} />}
          title="All quiet."
          text="Agents that need you will show up here."
        />
      ) : (
        <>
          <div className="session-filters">
            <label className="catalog-search">
              <Search size={14} />
              <input
                aria-label="Search inbox"
                placeholder="Search title or project…"
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setConfirming(false);
                }}
              />
            </label>
            <select
              aria-label="Inbox host"
              value={hostFilter}
              onChange={(e) => {
                setHostFilter(e.target.value);
                setConfirming(false);
              }}
            >
              <option value="">All hosts</option>
              {hosts.map(([key, label]) => (
                <option key={key} value={key}>
                  {label}
                </option>
              ))}
            </select>
            <button
              className="secondary"
              onClick={() => {
                setChecked(cleanupSelection(allRows));
                setConfirming(false);
              }}
            >
              <ListChecks size={13} /> Clean up
            </button>
          </div>
          <div className="session-select-all">
            <label>
              <input
                type="checkbox"
                checked={
                  visibleRows.length > 0 &&
                  selected.length === visibleRows.length
                }
                onChange={(e) => {
                  setChecked(
                    e.target.checked
                      ? visibleRows.map((row) => row.panel.id)
                      : [],
                  );
                  setConfirming(false);
                }}
              />{" "}
              Select visible ({visibleRows.length})
            </label>
            {selected.length > 0 && <span>{selected.length} selected</span>}
          </div>
          <div className="inbox-groups">
            {visibleTasks.length > 0 && (
              <div className="inbox-group">
                <div className="inbox-group-head">
                  <span>Orchestrator</span>
                  <span className="count">{visibleTasks.length}</span>
                </div>
                {visibleTasks.map((row) => (
                  <div className="session-row" key={row.key}>
                    <button
                      className="inbox-row-open"
                      title={`Open ${row.title} in the Orchestrator`}
                      aria-label={`Open task ${row.title}`}
                      onClick={row.open}
                    >
                      <strong>{row.title}</strong>
                      <small>{row.reason}</small>
                    </button>
                  </div>
                ))}
              </div>
            )}
            {visibleGroups
              .filter((group) => group.rows.length > 0)
              .map((group) => (
                <div className="inbox-group" key={group.key}>
                  <div className="inbox-group-head">
                    <span>{group.label}</span>
                    <span className="count">{group.rows.length}</span>
                  </div>
                  {group.rows.map((row) => {
                    const hostKey = groupKey(row.workspace.connection);
                    const hostLabel = groupLabel(hostKey, connectionProfiles);
                    const HostIcon = hostKey === LOCAL_GROUP ? Server : Globe;
                    const elapsed =
                      row.since != null
                        ? formatElapsed(Date.now() - row.since)
                        : null;
                    return (
                      <div className="session-row" key={row.panel.id}>
                        <input
                          aria-label={`Select ${row.panel.title} in ${row.workspace.name}`}
                          type="checkbox"
                          checked={checked.includes(row.panel.id)}
                          onChange={(e) =>
                            toggle(row.panel.id, e.target.checked)
                          }
                        />
                        <Icon kind={row.panel.kind} agent={row.panel.agent} />
                        {/* The row itself is the way back into the session:
                            the project leads, because that is what tells two
                            "Claude Code" rows apart, and the icon already
                            names the agent. */}
                        <button
                          className="inbox-row-open"
                          title={`Open ${row.panel.title} in ${row.workspace.name}`}
                          aria-label={`Jump to ${row.panel.title} in ${row.workspace.name}`}
                          onClick={() => jump(row)}
                        >
                          <strong>
                            {row.workspace.name}
                            {manyHosts && (
                              <span
                                className="inbox-row-host"
                                title={hostLabel}
                              >
                                <HostIcon size={11} />
                              </span>
                            )}
                          </strong>
                          <small>{row.panel.title}</small>
                        </button>
                        {elapsed && (
                          <span className="inbox-row-time">{elapsed}</span>
                        )}
                        {group.key === "done" && (
                          <button
                            title={`Mark ${row.panel.title} seen`}
                            aria-label={`Mark ${row.panel.title} seen`}
                            onClick={() => markSeen(row.panel.id)}
                          >
                            <Check size={14} />
                          </button>
                        )}
                      </div>
                    );
                  })}
                </div>
              ))}
          </div>
          {confirming ? (
            <div className="delete-confirm">
              <p>
                End these {selected.length} selected sessions? Running commands
                will stop. Project files will stay on disk.
              </p>
              <button onClick={() => setConfirming(false)}>Cancel</button>
              <button
                className="danger"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  try {
                    await ws.endSessions(
                      selected.map(({ workspace, panel }) => ({
                        workspace,
                        panel,
                      })),
                    );
                    setChecked([]);
                    setConfirming(false);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy ? "Closing…" : `End ${selected.length} sessions`}
              </button>
            </div>
          ) : selected.length > 0 ? (
            <button
              className="danger session-close-button"
              onClick={() => setConfirming(true)}
            >
              <Trash2 size={13} /> End selected
            </button>
          ) : null}
        </>
      )}
    </PageFrame>
  );
}
