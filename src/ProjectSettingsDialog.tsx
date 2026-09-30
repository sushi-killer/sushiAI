import { useCallback, useEffect, useState } from "react";
import { Pencil, RefreshCw, Trash2 } from "lucide-react";
import type { ConnectionProfile, Project, ProjectHostReadiness } from "./types";
import { ProjectMcpServersTab } from "./ProjectMcpServersTab";

type Tab = "General" | "Environment" | "MCP servers" | "Hosts";

export function ProjectSettingsDialog({
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
  const [project, setProject] = useState<Project | null>(null);
  const [tab, setTab] = useState<Tab>("General");
  const [name, setName] = useState(workspaceName);
  const [editingName, setEditingName] = useState(false);
  const [domain, setDomain] = useState("");
  const [error, setError] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);
  const [busy, setBusy] = useState(false);
  const [hostRows, setHostRows] = useState<ConnectionProfile[]>([]);
  const [hostMatrix, setHostMatrix] = useState<
    Record<string, ProjectHostReadiness>
  >({});
  const [hostBusy, setHostBusy] = useState("");
  const [overrideDrafts, setOverrideDrafts] = useState<Record<string, string>>(
    {},
  );
  const [refreshKey, setRefreshKey] = useState(0);

  const load = useCallback(async () => {
    if (!window.bridge) return;
    setBusy(true);
    setError("");
    try {
      const remoteInfo = await window.bridge.projectInspect(endpoint, {
        operation: "git_remote",
        root: cwd,
      });
      setProject(
        remoteInfo?.remote
          ? await window.bridge.projectsResolve({
              remote: remoteInfo.remote,
              endpoint: endpoint || "local",
            })
          : null,
      );
      setHostRows(await window.bridge.connectionsList());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setProject(null);
    } finally {
      setBusy(false);
    }
  }, [cwd, endpoint]);

  useEffect(() => {
    setName(workspaceName);
    setEditingName(false);
  }, [workspaceName]);
  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: Project) {
    if (!window.bridge) return;
    const saved = await window.bridge.projectsUpsert(next);
    setProject(saved);
  }

  async function checkHost(host: string) {
    if (!window.bridge || !project) return;
    setHostBusy(host);
    try {
      const matrix = await window.bridge.projectHostCheck(
        project.id,
        host,
        host === endpoint ? cwd : undefined,
      );
      setHostMatrix((current) => ({ ...current, [host]: matrix }));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setHostBusy("");
    }
  }

  async function changeTrust(host: string, trusted: boolean) {
    if (!window.bridge || !project) return;
    setHostBusy(host);
    try {
      await window.bridge.projectHostTrust(project.id, host, trusted);
      setProject(await window.bridge.projectsGet(project.id));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setHostBusy("");
    }
  }

  async function saveHostOverrides(host: string) {
    if (!window.bridge || !project) return;
    try {
      const overrides = JSON.parse(overrideDrafts[host] || "{}");
      if (
        !overrides ||
        typeof overrides !== "object" ||
        Array.isArray(overrides)
      )
        throw new Error("Overrides must be a JSON object.");
      await window.bridge.projectHostOverrides(project.id, host, overrides);
      setProject(await window.bridge.projectsGet(project.id));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  async function rename() {
    const value = name.trim();
    if (!value) return;
    try {
      await onRename(value);
      setEditingName(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  return (
    <div className="project-settings-shell">
      <aside className="project-settings-rail">
        <div className="project-settings-identity">
          <span className="dialog-eyebrow">PROJECT</span>
          {editingName ? (
            <div className="project-settings-rename">
              <input
                aria-label="Project name"
                value={name}
                maxLength={80}
                onChange={(event) => setName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void rename();
                  if (event.key === "Escape") setEditingName(false);
                }}
              />
              <button className="primary" onClick={() => void rename()}>
                Save
              </button>
            </div>
          ) : (
            <div className="project-settings-title">
              <strong title={workspaceName}>{workspaceName}</strong>
              <button
                className="icon-button"
                aria-label="Rename project"
                title="Rename project"
                onClick={() => setEditingName(true)}
              >
                <Pencil size={13} />
              </button>
            </div>
          )}
          <small>
            {remote ? "SSH host" : "This Mac"} · {sessionCount}{" "}
            {sessionCount === 1 ? "session" : "sessions"}
          </small>
        </div>
        <nav aria-label="Project settings" className="project-settings-nav">
          {(["General", "Environment", "MCP servers", "Hosts"] as Tab[]).map(
            (item) => (
              <button
                key={item}
                className={tab === item ? "selected" : ""}
                aria-current={tab === item ? "page" : undefined}
                onClick={() => setTab(item)}
              >
                {item}
              </button>
            ),
          )}
        </nav>
        <button
          className="project-settings-close danger"
          onClick={() => setConfirmClose(true)}
        >
          <Trash2 size={14} /> Close project…
        </button>
      </aside>

      <main className="project-settings-content">
        <header className="project-settings-content-header">
          <div>
            <h2>{tab}</h2>
            <span>{cwd}</span>
          </div>
          <button
            className="icon-button"
            aria-label="Refresh project settings"
            title="Refresh"
            disabled={busy}
            onClick={() => {
              setRefreshKey((value) => value + 1);
              void load();
            }}
          >
            <RefreshCw size={14} />
          </button>
        </header>
        {error && (
          <p role="alert" className="settings-error">
            {error}
          </p>
        )}
        {tab === "General" &&
          (project ? (
            <div className="project-general">
              <section className="project-general-card">
                <h3>Git</h3>
                <label>
                  Remote
                  <input value={project.git.url} readOnly />
                </label>
                <label>
                  Default branch
                  <input
                    value={project.git.defaultBranch}
                    placeholder="main"
                    onChange={(event) =>
                      setProject({
                        ...project,
                        git: {
                          ...project.git,
                          defaultBranch: event.target.value,
                        },
                      })
                    }
                    onBlur={() => void save(project)}
                  />
                </label>
              </section>
              <section className="project-general-card">
                <h3>Sessions</h3>
                <label>
                  Claude account
                  <input
                    value={project.sessions.claudeAccount || ""}
                    placeholder="Default account"
                    onChange={(event) =>
                      setProject({
                        ...project,
                        sessions: {
                          ...project.sessions,
                          claudeAccount: event.target.value || undefined,
                        },
                      })
                    }
                    onBlur={() => void save(project)}
                  />
                </label>
                <label>
                  Backend
                  <select
                    value={project.sessions.backend || "herdr"}
                    onChange={(event) =>
                      void save({
                        ...project,
                        sessions: {
                          ...project.sessions,
                          backend: event.target.value as "herdr" | "local",
                        },
                      })
                    }
                  >
                    <option value="herdr">Herdr</option>
                    <option value="local">Local</option>
                  </select>
                </label>
              </section>
              <section className="project-general-card">
                <h3>Setup</h3>
                <label>
                  Install command
                  <input
                    value={project.setup.install}
                    placeholder="e.g. npm install"
                    onChange={(event) =>
                      setProject({
                        ...project,
                        setup: {
                          ...project.setup,
                          install: event.target.value,
                        },
                      })
                    }
                    onBlur={() => void save(project)}
                  />
                </label>
                <label>
                  Check command
                  <input
                    value={project.setup.check}
                    placeholder="e.g. npm test"
                    onChange={(event) =>
                      setProject({
                        ...project,
                        setup: { ...project.setup, check: event.target.value },
                      })
                    }
                    onBlur={() => void save(project)}
                  />
                </label>
              </section>
              <section className="project-general-card">
                <h3>Network</h3>
                <p>Domains this project may reach.</p>
                <div className="project-domain-chips">
                  {project.network.allowedDomains.map((item) => (
                    <button
                      key={item}
                      title={`Remove ${item}`}
                      onClick={() =>
                        void save({
                          ...project,
                          network: {
                            allowedDomains:
                              project.network.allowedDomains.filter(
                                (value) => value !== item,
                              ),
                          },
                        })
                      }
                    >
                      {item} <span>×</span>
                    </button>
                  ))}
                </div>
                <form
                  onSubmit={(event) => {
                    event.preventDefault();
                    const value = domain.trim();
                    if (
                      value &&
                      !project.network.allowedDomains.includes(value)
                    )
                      void save({
                        ...project,
                        network: {
                          allowedDomains: [
                            ...project.network.allowedDomains,
                            value,
                          ],
                        },
                      });
                    setDomain("");
                  }}
                >
                  <input
                    aria-label="Allowed domain"
                    placeholder="example.com"
                    value={domain}
                    onChange={(event) => setDomain(event.target.value)}
                  />
                  <button className="secondary">Add domain</button>
                </form>
              </section>
            </div>
          ) : (
            <p className="settings-muted">
              This checkout has no git remote, so it is not linked to a project
              yet.
            </p>
          ))}
        {tab === "Environment" && (
          <div className="project-settings-placeholder">
            <h3>Environment</h3>
            <p>Project variables and secrets will appear here.</p>
          </div>
        )}
        {tab === "MCP servers" &&
          (project ? (
            <ProjectMcpServersTab
              project={project}
              cwd={cwd}
              endpoint={endpoint}
              remote={remote}
              onChange={save}
              key={refreshKey}
            />
          ) : (
            <p className="settings-muted">
              Open a project with a git remote to manage its MCP servers.
            </p>
          ))}
        {tab === "Hosts" &&
          (project ? (
            <div className="project-settings-placeholder">
              <h3>Project hosts</h3>
              <p>
                Configured SSH hosts and their trust settings are stored with
                this project.
              </p>
              {hostRows.length === 0 ? (
                <p>No SSH hosts are configured.</p>
              ) : (
                hostRows.map((host) => {
                  const key = `ssh:${host.id}`;
                  const matrix = hostMatrix[key];
                  const trusted = !!project.hosts?.[key]?.trusted;
                  return (
                    <section className="project-host-row" key={host.id}>
                      <div className="project-host-heading">
                        <strong>{host.name}</strong>
                        <span>{trusted ? "Trusted" : "Not trusted"}</span>
                      </div>
                      {matrix ? (
                        <div className="project-host-matrix">
                          {[
                            [
                              "Checkout",
                              matrix.checkout.ok
                                ? "Matches project remote"
                                : "Remote does not match",
                              matrix.checkout.ok,
                            ],
                            [
                              "Setup",
                              matrix.setup.ok ? "Configured" : "Not configured",
                              matrix.setup.ok,
                            ],
                            [
                              "CLIs",
                              `${matrix.clis.claude.installed ? "Claude" : "No Claude"} · ${matrix.clis.codex.installed ? "Codex" : "No Codex"}`,
                              matrix.clis.claude.installed ||
                                matrix.clis.codex.installed,
                            ],
                            [
                              "MCP",
                              `${matrix.mcp.count} configured`,
                              matrix.mcp.ok,
                            ],
                            [
                              "Secrets",
                              `${matrix.secrets.count} ready`,
                              matrix.secrets.ok,
                            ],
                          ].map(([label, value, ok]) => (
                            <div
                              className={
                                ok
                                  ? "project-host-ready"
                                  : "project-host-missing"
                              }
                              key={String(label)}
                            >
                              <span>{label}</span>
                              <strong>
                                {ok ? "Ready" : "Needs attention"}
                              </strong>
                              <small>{value}</small>
                            </div>
                          ))}
                        </div>
                      ) : null}
                      <label className="project-host-overrides">
                        Per-host overrides (JSON)
                        <textarea
                          value={
                            overrideDrafts[key] ??
                            JSON.stringify(
                              project.hosts?.[key]?.overrides ?? {},
                              null,
                              2,
                            )
                          }
                          onChange={(event) =>
                            setOverrideDrafts((current) => ({
                              ...current,
                              [key]: event.target.value,
                            }))
                          }
                          spellCheck={false}
                        />
                      </label>
                      <button
                        className="secondary"
                        onClick={() => void saveHostOverrides(key)}
                      >
                        Save overrides
                      </button>
                      <div className="project-host-actions">
                        <button
                          className="secondary"
                          disabled={!!hostBusy}
                          onClick={() => void checkHost(key)}
                        >
                          {hostBusy === key ? "Checking…" : "Check readiness"}
                        </button>
                        <button
                          className={trusted ? "danger" : "primary"}
                          disabled={!!hostBusy}
                          onClick={() => void changeTrust(key, !trusted)}
                        >
                          {trusted ? "Revoke trust" : "Trust host"}
                        </button>
                      </div>
                    </section>
                  );
                })
              )}
            </div>
          ) : (
            <p className="settings-muted">
              Open a project with a git remote to manage its hosts.
            </p>
          ))}
      </main>
      {confirmClose && (
        <div className="workspace-confirm">
          <div className="workspace-confirm-box">
            <div className="dialog-eyebrow">CLOSE PROJECT</div>
            <h2>Close {workspaceName}?</h2>
            <p>
              The project disappears from the sidebar and its {sessionCount}{" "}
              {sessionCount === 1 ? "session stops" : "sessions stop"}.
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
                Close project
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
