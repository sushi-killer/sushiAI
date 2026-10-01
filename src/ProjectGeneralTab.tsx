import { useEffect, useRef, useState } from "react";
import type { ClaudeAccount, CodexAccount, Project } from "./types";
import { ProjectPage } from "./ProjectPage";

function Field({
  label,
  hint,
  first,
  children,
}: {
  label: string;
  hint: string;
  first?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className={`pd-field${first ? " first" : ""}`}>
      <span className="pd-field-label">
        <strong>{label}</strong>
        <span>{hint}</span>
      </span>
      {children}
    </div>
  );
}

export function ProjectGeneralTab({
  project,
  setProject,
  save,
  name,
  onRename,
}: {
  project: Project | null;
  setProject(project: Project): void;
  save(project: Project): Promise<void>;
  /** The workspace's name; renaming it renames the project with it. */
  name: string;
  onRename(name: string): Promise<void>;
}) {
  const [draft, setDraft] = useState(name);
  const [renameError, setRenameError] = useState("");
  useEffect(() => setDraft(name), [name]);
  const cancelled = useRef(false);
  async function commitName() {
    if (cancelled.current) {
      cancelled.current = false;
      return setDraft(name);
    }
    const value = draft.trim();
    if (!value || value === name) return setDraft(name);
    try {
      setRenameError("");
      await onRename(value);
    } catch (reason) {
      setDraft(name);
      setRenameError(reason instanceof Error ? reason.message : String(reason));
    }
  }
  const [accounts, setAccounts] = useState<ClaudeAccount[]>([]);
  const [codexAccounts, setCodexAccounts] = useState<CodexAccount[]>([]);
  const [domain, setDomain] = useState("");
  const [adding, setAdding] = useState(false);
  useEffect(() => {
    window.bridge
      ?.claudeAccountsList()
      .then(setAccounts)
      .catch(() => {});
    window.bridge
      ?.codexAccountsList()
      .then(setCodexAccounts)
      .catch(() => {});
  }, []);
  const subtitle =
    project && !project.git.url
      ? "This folder is the project. Add a git remote to run it on other hosts."
      : "One repository and one setup for every host. A host without a checkout clones it on its first run.";
  if (!project)
    return (
      <ProjectPage title="General" subtitle={subtitle}>
        <p className="pd-empty">
          This checkout has no git remote, so it is not linked to a project yet.
        </p>
      </ProjectPage>
    );
  const domains = project.network.allowedDomains;
  const commit = () => void save(project);
  const addDomain = () => {
    const value = domain.trim();
    setDomain("");
    setAdding(false);
    if (value && !domains.includes(value))
      void save({
        ...project,
        network: { allowedDomains: [...domains, value] },
      });
  };
  return (
    <ProjectPage title="General" subtitle={subtitle}>
      <h3 className="pd-group">Project</h3>
      <Field label="Name" hint="Shown in the sidebar and the + picker.">
        <input
          className="pd-input"
          aria-label="Project name"
          value={draft}
          maxLength={80}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={() => void commitName()}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
            if (event.key === "Escape") {
              // Cancels the edit only: the dialog stays open.
              event.stopPropagation();
              cancelled.current = true;
              setDraft(name);
              event.currentTarget.blur();
            }
          }}
        />
        {renameError && (
          <span role="alert" className="pd-alert">
            {renameError}
          </span>
        )}
      </Field>
      <h3 className="pd-group">Repository</h3>
      <Field
        label="Git remote"
        hint="Matches this project to its checkout on any host."
      >
        <input
          className="pd-input"
          aria-label="Git remote"
          value={project.git.url}
          placeholder="git@host:org/repo.git"
          readOnly
        />
      </Field>
      <Field
        label="Default branch"
        hint="Tasks branch from here and land back into it."
      >
        <input
          className="pd-input"
          aria-label="Default branch"
          value={project.git.defaultBranch}
          placeholder="main"
          onChange={(event) =>
            setProject({
              ...project,
              git: { ...project.git, defaultBranch: event.target.value },
            })
          }
          onBlur={commit}
        />
      </Field>
      <h3 className="pd-group">Sessions</h3>
      <Field
        label="Claude Code account"
        hint="For new sessions and tasks. Change it for one session in +."
      >
        <select
          className="pd-input"
          aria-label="Claude Code account"
          value={project.sessions.claudeAccount || ""}
          onChange={(event) =>
            void save({
              ...project,
              sessions: {
                ...project.sessions,
                claudeAccount: event.target.value || undefined,
              },
            })
          }
        >
          <option value="">Default account</option>
          {accounts.map((account) => (
            <option key={account.id} value={account.id}>
              {account.label}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Codex account"
        hint="For new sessions. Change it for one session in +."
      >
        <select
          className="pd-input"
          aria-label="Codex account"
          value={project.sessions.codexAccount || ""}
          onChange={(event) =>
            void save({
              ...project,
              sessions: {
                ...project.sessions,
                codexAccount: event.target.value || undefined,
              },
            })
          }
        >
          <option value="">Default account</option>
          {codexAccounts.map((account) => (
            <option key={account.id} value={account.id}>
              {account.label}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Session backend"
        hint="Herdr keeps sessions running after the app closes."
      >
        <select
          className="pd-input"
          aria-label="Session backend"
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
      </Field>
      <h3 className="pd-group">Setup</h3>
      <Field
        label="Install"
        hint="Runs on the first run, and again when a lock file changes."
      >
        <input
          className="pd-input"
          aria-label="Install command"
          value={project.setup.install}
          placeholder="npm install"
          onChange={(event) =>
            setProject({
              ...project,
              setup: { ...project.setup, install: event.target.value },
            })
          }
          onBlur={commit}
        />
      </Field>
      <Field
        label="Check before start"
        hint="Optional. A failing check stops the run before an agent starts."
      >
        <input
          className="pd-input"
          aria-label="Check command"
          value={project.setup.check}
          placeholder="npm test -- --bail"
          onChange={(event) =>
            setProject({
              ...project,
              setup: { ...project.setup, check: event.target.value },
            })
          }
          onBlur={commit}
        />
      </Field>
      <h3 className="pd-group">Network</h3>
      <div className="pd-field">
        <span className="pd-field-label">
          <strong>Agents may reach</strong>
          <span>
            The sandbox allowlist on every host. Everything else is blocked.
          </span>
        </span>
        <div className="pd-chips">
          {domains.map((item) => (
            <button
              key={item}
              type="button"
              className="pd-chip"
              title={`Remove ${item}`}
              onClick={() =>
                void save({
                  ...project,
                  network: {
                    allowedDomains: domains.filter((value) => value !== item),
                  },
                })
              }
            >
              {item}
              <span className="pd-chip-x" aria-hidden>
                ×
              </span>
            </button>
          ))}
          {adding ? (
            <span className="pd-chip">
              <input
                autoFocus
                aria-label="Allowed domain"
                placeholder="example.com"
                value={domain}
                onChange={(event) => setDomain(event.target.value)}
                onBlur={addDomain}
                onKeyDown={(event) => {
                  if (event.key === "Enter") addDomain();
                  if (event.key === "Escape") {
                    setDomain("");
                    setAdding(false);
                  }
                }}
              />
            </span>
          ) : (
            <button
              type="button"
              className="pd-chip"
              onClick={() => setAdding(true)}
            >
              + Add
            </button>
          )}
        </div>
      </div>
    </ProjectPage>
  );
}
