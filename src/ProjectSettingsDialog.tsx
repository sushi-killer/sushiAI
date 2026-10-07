import { useCallback, useEffect, useState } from "react";
import { GitBranch, Globe, Lock, Server, Settings } from "lucide-react";
import type { ConnectionProfile, Project, Workspace } from "./types";
import { ProjectMcpServersTab } from "./ProjectMcpServersTab";
import { ProjectEnvironmentTab } from "./ProjectEnvironmentTab";
import { ProjectGeneralTab } from "./ProjectGeneralTab";
import { ProjectHostsTab } from "./ProjectHostsTab";
import { ProjectWorktreesTab } from "./ProjectWorktreesTab";
import { ProjectPage } from "./ProjectPage";
import { repoSlug } from "./projectPrepare";
import { takePendingTab } from "./lib/openSettings";

type Tab = "General" | "Environment" | "MCP servers" | "Worktrees" | "Hosts";
const TABS = [
  ["General", Settings],
  ["Environment", Lock],
  ["MCP servers", Server],
  ["Worktrees", GitBranch],
  ["Hosts", Globe],
] as const;

export function ProjectSettingsDialog({
  workspace,
  workspaces,
  onRename,
  onCloseWorkspace,
  onEndWorkspace,
}: {
  workspace: Workspace;
  workspaces: Workspace[];
  onRename: (name: string) => Promise<void>;
  onCloseWorkspace: () => Promise<void>;
  onEndWorkspace: (workspace: Workspace) => Promise<void>;
}) {
  const { cwd, connection: endpoint, name: workspaceName } = workspace;
  const remote = endpoint?.startsWith("ssh:") || false;
  const sessionCount = workspace.panels.length;
  const [project, setProject] = useState<Project | null>(null);
  const [tab, setTab] = useState<Tab>(() => takePendingTab(cwd, endpoint));
  const [error, setError] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);
  const [hostRows, setHostRows] = useState<ConnectionProfile[]>([]);

  const load = useCallback(async () => {
    if (!window.bridge) return;
    setError("");
    try {
      // Every workspace has a project: by its remote, else by its folder.
      setProject(
        await window.bridge.projectAttach({
          endpoint: endpoint || "local",
          cwd,
          name: workspaceName,
        }),
      );
      setHostRows(await window.bridge.connectionsList());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setProject(null);
    }
    // The name only seeds a project that does not exist yet.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cwd, endpoint]);

  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: Project) {
    if (!window.bridge) return;
    const saved = await window.bridge.projectsUpsert(next);
    setProject(saved);
  }

  /** The workspace and its project take the new name together. */
  async function rename(value: string) {
    await onRename(value);
    if (project) await save({ ...project, name: value });
  }

  // One name wherever the dialog opens from: every worktree and host of the
  // project shares the project's, while each workspace keeps its own.
  const projectName = project?.name || workspaceName;

  return (
    <div className="pd">
      <aside className="pd-rail">
        <div className="pd-head">
          <span className="pd-eyebrow">PROJECT</span>
          <strong className="pd-name" title={projectName}>
            {projectName}
          </strong>
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
            name={projectName}
            onRename={rename}
          />
        )}
        {tab === "Environment" && (
          <ProjectEnvironmentTab
            project={project}
            setProject={setProject}
            cwd={cwd}
            endpoint={endpoint}
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
        {tab === "Worktrees" && (
          <ProjectWorktreesTab
            project={project}
            cwd={cwd}
            endpoint={endpoint}
            hosts={hostRows}
            workspaces={workspaces}
            onEndWorkspace={onEndWorkspace}
          />
        )}
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
            <h2>Close {projectName}?</h2>
            <p>
              The project disappears from the sidebar and its {sessionCount}{" "}
              {sessionCount === 1 ? "session stops" : "sessions stop"}.
              {remote
                ? " Sessions on the host are closed."
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
