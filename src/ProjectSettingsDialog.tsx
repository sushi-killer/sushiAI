import { useCallback, useEffect, useState } from "react";
import { Globe, Lock, Server, Settings } from "lucide-react";
import type { ConnectionProfile, Project } from "./types";
import { ProjectMcpServersTab } from "./ProjectMcpServersTab";
import { ProjectEnvironmentTab } from "./ProjectEnvironmentTab";
import { ProjectGeneralTab } from "./ProjectGeneralTab";
import { ProjectHostsTab } from "./ProjectHostsTab";
import { ProjectPage } from "./ProjectPage";
import { repoSlug } from "./projectPrepare";
import { takePendingTab } from "./app/openSettings";

type Tab = "General" | "Environment" | "MCP servers" | "Hosts";
const TABS = [
  ["General", Settings],
  ["Environment", Lock],
  ["MCP servers", Server],
  ["Hosts", Globe],
] as const;

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
  const [tab, setTab] = useState<Tab>(() => takePendingTab(cwd, endpoint));
  const [name, setName] = useState(workspaceName);
  const [editingName, setEditingName] = useState(false);
  const [error, setError] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);
  const [hostRows, setHostRows] = useState<ConnectionProfile[]>([]);

  const load = useCallback(async () => {
    if (!window.bridge) return;
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
    <div className="pd">
      <aside className="pd-rail">
        <div className="pd-head">
          <span className="pd-eyebrow">PROJECT</span>
          {editingName ? (
            <div className="pd-rename">
              <input
                aria-label="Project name"
                value={name}
                maxLength={80}
                autoFocus
                onChange={(event) => setName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void rename();
                  if (event.key === "Escape") setEditingName(false);
                }}
              />
              <button onClick={() => void rename()}>Save</button>
            </div>
          ) : (
            <div className="pd-name-row">
              <strong title={workspaceName}>{workspaceName}</strong>
              <button
                title={`Rename ${workspaceName}`}
                onClick={() => setEditingName(true)}
              >
                Rename
              </button>
            </div>
          )}
          <span className="pd-remote">
            {project
              ? repoSlug(project.git.url)
              : `${remote ? "SSH host" : "This Mac"} · ${sessionCount} ${sessionCount === 1 ? "session" : "sessions"}`}
          </span>
        </div>
        <nav aria-label="Project settings" className="pd-nav">
          {TABS.map(([item, Icon]) => (
            <button
              key={item}
              className={tab === item ? "selected" : ""}
              aria-current={tab === item ? "page" : undefined}
              onClick={() => setTab(item)}
            >
              <Icon size={15} aria-hidden />
              {item}
            </button>
          ))}
        </nav>
        <button
          className="pd-close-project"
          onClick={() => setConfirmClose(true)}
        >
          Close project…
        </button>
      </aside>

      <main className="pd-main">
        {error && (
          <p role="alert" className="pd-alert">
            {error}
          </p>
        )}
        {tab === "General" && (
          <ProjectGeneralTab
            project={project}
            setProject={setProject}
            save={save}
          />
        )}
        {tab === "Environment" && (
          <ProjectEnvironmentTab
            project={project}
            setProject={setProject}
            gitRemote={project?.git.url || ""}
            projectName={workspaceName}
            cwd={cwd}
            remote={remote}
            targets={hostRows}
          />
        )}
        {tab === "MCP servers" &&
          (project ? (
            <ProjectMcpServersTab
              project={project}
              cwd={cwd}
              endpoint={endpoint}
              remote={remote}
              hosts={hostRows}
              onProject={setProject}
            />
          ) : (
            <ProjectPage
              title="MCP servers"
              subtitle="Every run on every host gets these servers."
            >
              <p className="pd-empty">
                Open a project with a git remote to manage its MCP servers.
              </p>
            </ProjectPage>
          ))}
        {tab === "Hosts" && (
          <ProjectHostsTab
            project={project}
            hosts={hostRows}
            cwd={cwd}
            endpoint={endpoint}
            remote={remote}
            onProject={setProject}
          />
        )}
      </main>
      {confirmClose && (
        <div className="pd-confirm">
          <div className="pd-confirm-box">
            <div className="dialog-eyebrow">CLOSE PROJECT</div>
            <h2>Close {workspaceName}?</h2>
            <p>
              The project disappears from the sidebar and its {sessionCount}{" "}
              {sessionCount === 1 ? "session stops" : "sessions stop"}.
              {remote
                ? " Herdr sessions on the host are closed."
                : " Local terminals stop and unsaved file edits are lost."}
            </p>
            <div className="pd-confirm-actions">
              <button
                className="ui-button secondary"
                onClick={() => setConfirmClose(false)}
              >
                Cancel
              </button>
              <button
                className="ui-button primary"
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
