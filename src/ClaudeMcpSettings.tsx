import { useEffect, useMemo, useState } from "react";
import { Check, Pencil, RefreshCw, ShieldOff, Trash2 } from "lucide-react";
import type { ClaudeMcpServer, ClaudePlugin, SkillCatalogItem } from "./types";

type Usage = { here: number; total: number; skills: number };

/** One line of the merged control table: a project control plus its usage. */
type Row =
  | {
      kind: "MCP";
      name: string;
      source: ClaudeMcpServer["source"];
      sourceLabel: string;
      disabled: boolean;
      /** MCP servers have no skill usage. */
      usage: null;
    }
  | {
      kind: "Plugin";
      name: string;
      source: ClaudePlugin["source"];
      sourceLabel: string;
      disabled: boolean;
      usage: Usage | null;
    };

function projectUsage(item: SkillCatalogItem, cwd: string) {
  return (item.recentUses || []).filter((use) => use.project === cwd).length;
}

/** Aggregates the scanned skills of one plugin into the numbers the row shows. */
function usageFor(
  plugin: ClaudePlugin,
  skills: SkillCatalogItem[],
  cwd: string,
): Usage {
  const mine = skills.filter((skill) => skill.plugin === plugin.name);
  return {
    skills: mine.length,
    here: mine.reduce((sum, skill) => sum + projectUsage(skill, cwd), 0),
    total: mine.reduce((sum, skill) => sum + (skill.usageCount || 0), 0),
  };
}

// The meta line only needs enough path to identify the workspace; the full
// path stays in the tooltip.
function shortPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join("/")}` : path || "—";
}

export function ClaudeMcpSettings({
  cwd,
  endpoint,
  remote,
  workspaceName,
  sessionCount,
  onRename,
  onCloseWorkspace,
}: {
  cwd: string;
  endpoint?: string;
  remote: boolean;
  workspaceName: string;
  sessionCount: number;
  onRename: (name: string) => Promise<void>;
  onCloseWorkspace: () => Promise<void>;
}) {
  const [name, setName] = useState(workspaceName);
  const [editingName, setEditingName] = useState(false);
  const [servers, setServers] = useState<ClaudeMcpServer[]>([]);
  const [plugins, setPlugins] = useState<ClaudePlugin[]>([]);
  const [skills, setSkills] = useState<SkillCatalogItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [busyRow, setBusyRow] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [loading, setLoading] = useState(true);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [error, setError] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);

  useEffect(() => {
    setName(workspaceName);
    setEditingName(false);
  }, [workspaceName]);

  async function load() {
    if (!window.bridge) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError("");
    const [mcpResult, pluginResult] = await Promise.allSettled([
      window.bridge.claudeMcpList(cwd, endpoint),
      window.bridge.claudePluginsList(cwd, endpoint),
    ]);
    const errors: string[] = [];
    if (mcpResult.status === "fulfilled") setServers(mcpResult.value.servers);
    else
      errors.push(
        `MCP: ${mcpResult.reason instanceof Error ? mcpResult.reason.message : String(mcpResult.reason)}`,
      );
    if (pluginResult.status === "fulfilled")
      setPlugins(pluginResult.value.plugins);
    else
      errors.push(
        `Plugins: ${pluginResult.reason instanceof Error ? pluginResult.reason.message : String(pluginResult.reason)}`,
      );
    setError(errors.join(" · "));
    setLoading(false);
  }

  async function loadAnalytics() {
    if (remote || !window.bridge?.catalog) {
      setSkills([]);
      return;
    }
    setAnalyticsLoading(true);
    try {
      setSkills(await window.bridge.catalog("skills", { force: true }));
    } catch {
      setSkills([]);
    } finally {
      setAnalyticsLoading(false);
    }
  }

  useEffect(() => {
    void load();
    void loadAnalytics();
  }, [cwd, endpoint, remote]);

  async function reload() {
    await Promise.all([load(), loadAnalytics()]);
  }

  const rows: Row[] = useMemo(
    () => [
      ...servers.map((server) => ({
        ...server,
        kind: "MCP" as const,
        usage: null,
      })),
      ...plugins.map((plugin) => ({
        ...plugin,
        kind: "Plugin" as const,
        usage: remote ? null : usageFor(plugin, skills, cwd),
      })),
    ],
    [plugins, servers, skills, cwd, remote],
  );

  async function toggle(row: Row) {
    if (!window.bridge || busy) return;
    setBusy(true);
    setBusyRow(`${row.kind}:${row.name}`);
    setError("");
    try {
      if (row.kind === "MCP") {
        const result = await window.bridge.claudeMcpToggle({
          cwd,
          endpoint,
          name: row.name,
          source: row.source,
          disabled: !row.disabled,
        });
        setServers(result.servers);
      } else {
        const result = await window.bridge.claudePluginsToggle({
          cwd,
          endpoint,
          name: row.name,
          disabled: !row.disabled,
        });
        setPlugins(result.plugins);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyRow("");
    }
  }

  function startRename() {
    setName(workspaceName);
    setEditingName(true);
  }

  function cancelRename() {
    setName(workspaceName);
    setEditingName(false);
  }

  async function rename() {
    const value = name.trim();
    if (!value || renaming) return;
    setRenaming(true);
    setError("");
    try {
      await onRename(value);
      setEditingName(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setRenaming(false);
    }
  }

  return (
    <div className="workspace-controls-settings">
      <div className="dialog-eyebrow">WORKSPACE</div>

      <div className="workspace-controls-heading">
        {editingName ? (
          <input
            autoFocus
            className="workspace-name-input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void rename();
              }
              if (event.key === "Escape") {
                event.preventDefault();
                cancelRename();
              }
            }}
            aria-label="Workspace name"
            maxLength={80}
            spellCheck={false}
          />
        ) : (
          <h2>
            {workspaceName}
            <button
              className="workspace-rename-toggle"
              title="Rename workspace"
              aria-label="Rename workspace"
              onClick={startRename}
            >
              <Pencil size={13} />
            </button>
          </h2>
        )}
        <div className="workspace-heading-actions">
          {editingName ? (
            <>
              <button
                className="secondary"
                disabled={renaming}
                onClick={cancelRename}
              >
                Cancel
              </button>
              <button
                className="primary"
                disabled={renaming}
                onClick={() => void rename()}
              >
                {renaming ? "Saving…" : "Save"}
              </button>
            </>
          ) : (
            <button
              className="icon-button"
              title="Refresh workspace controls"
              aria-label="Refresh workspace controls"
              disabled={loading || busy}
              onClick={() => void reload()}
            >
              <RefreshCw size={13} />
            </button>
          )}
        </div>
      </div>

      <p className="workspace-meta">
        {remote ? (
          <span className="workspace-host">SSH host</span>
        ) : (
          <span>This Mac</span>
        )}
        <span className="sep">·</span>
        <span>
          {sessionCount} {sessionCount === 1 ? "session" : "sessions"}
        </span>
        <span className="sep">·</span>
        <span className="workspace-cwd" title={cwd}>
          {shortPath(cwd)}
        </span>
      </p>

      {loading ? (
        <p className="settings-muted">
          Looking for workspace controls{remote ? " over SSH" : ""}…
        </p>
      ) : (
        <>
          {error ? (
            <p role="alert" className="settings-error">
              {error}
            </p>
          ) : null}
          {remote ? (
            <div className="workspace-control-empty">
              <ShieldOff size={14} />
              <span>
                Project usage analytics are not available over SSH yet.
              </span>
            </div>
          ) : analyticsLoading ? (
            <p className="settings-muted">Building project usage stats…</p>
          ) : null}
          {rows.length ? (
            <div className="workspace-scroll">
              <table className="workspace-table">
                <thead>
                  <tr>
                    <th>Integration</th>
                    <th>Source</th>
                    <th className="num">Uses · 30d</th>
                    <th className="num">Total</th>
                    <th className="acc">Access</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <tr key={`${row.kind}:${row.name}`}>
                      <td>
                        <span className="workspace-cell">
                          <strong title={row.name}>{row.name}</strong>
                          <span className="workspace-kind">{row.kind}</span>
                        </span>
                      </td>
                      <td className="workspace-src">{row.sourceLabel}</td>
                      <td className="workspace-uses">
                        {row.usage ? (
                          row.usage.skills ? (
                            <span className="workspace-bar">
                              {row.usage.here > 0 ? (
                                <i
                                  style={{
                                    width: `${Math.max(4, Math.round((row.usage.here / 10) * 46))}px`,
                                  }}
                                />
                              ) : null}
                              <span>{row.usage.here}</span>
                            </span>
                          ) : (
                            <span className="workspace-src">
                              No skill usage data
                            </span>
                          )
                        ) : (
                          <span className="workspace-dash">
                            {analyticsLoading && row.kind === "Plugin"
                              ? "…"
                              : "—"}
                          </span>
                        )}
                      </td>
                      <td className="workspace-num">
                        {row.usage?.skills ? (
                          row.usage.total
                        ) : (
                          <span className="workspace-dash">—</span>
                        )}
                      </td>
                      <td>
                        <span className="workspace-access">
                          <input
                            type="checkbox"
                            checked={!row.disabled}
                            disabled={busy}
                            aria-label={`${row.disabled ? "Enable" : "Disable"} ${row.name}`}
                            onChange={() => void toggle(row)}
                          />
                          <span>
                            {busyRow === `${row.kind}:${row.name}`
                              ? "…"
                              : row.disabled
                                ? "Off"
                                : "On"}
                          </span>
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="workspace-control-empty">
              <Check size={14} />
              <span>No project controls found.</span>
            </div>
          )}
          <p className="workspace-note">
            Written to <b>~/.claude.json</b>, <b>.mcp.json</b>,{" "}
            <b>.claude/settings.json</b> and <b>.claude/settings.local.json</b>{" "}
            for this project{remote ? " on the SSH host" : ""}. Uses counts
            plugin skill calls for this project in the last 30 days; MCP servers
            have no usage.
          </p>
        </>
      )}

      <div className="workspace-controls-footer">
        <span className="spacer" />
        <span>Changes apply to new sessions.</span>
      </div>

      <div className="settings-divider" />
      <div className="workspace-close-row">
        <button
          className="danger"
          disabled={busy}
          onClick={() => setConfirmClose(true)}
        >
          <Trash2 size={13} /> Close workspace and sessions
        </button>
      </div>

      {confirmClose ? (
        <div
          className="workspace-confirm"
          onClick={(event) => {
            if (event.target === event.currentTarget) setConfirmClose(false);
          }}
        >
          <div className="workspace-confirm-box">
            <div className="dialog-eyebrow">CLOSE WORKSPACE</div>
            <h2>Close {workspaceName}?</h2>
            <p>
              The workspace disappears from the sidebar and its {sessionCount}{" "}
              {sessionCount === 1 ? "session" : "sessions"} stop.
              {remote
                ? " Herdr sessions on the host are closed."
                : " Local terminals stop and unsaved file edits are lost."}
            </p>
            <div className="workspace-confirm-actions">
              <button
                className="secondary"
                onClick={() => setConfirmClose(false)}
              >
                Cancel
              </button>
              <button
                className="danger"
                onClick={() => {
                  setConfirmClose(false);
                  void onCloseWorkspace();
                }}
              >
                Close workspace
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
