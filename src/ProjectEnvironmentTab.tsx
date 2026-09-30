import { useCallback, useEffect, useRef, useState } from "react";
import { Lock, Plus, Upload, X } from "lucide-react";
import type { Project } from "./types";
import {
  displayedEnvValue,
  isSecretEnvName,
  parseEnv,
  type ParsedEnvEntry,
} from "./projectEnv";

type ReviewEntry = ParsedEnvEntry & {
  secret: boolean;
  status: string;
  selected: boolean;
  override: boolean;
  overrideHost: string;
};

export function ProjectEnvironmentTab({
  gitRemote,
  projectName,
}: {
  gitRemote: string;
  projectName: string;
}) {
  const [project, setProject] = useState<Project | null>(null);
  const [error, setError] = useState("");
  const [review, setReview] = useState<ReviewEntry[] | null>(null);
  const [fileName, setFileName] = useState("");
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    if (!window.bridge || !gitRemote) return;
    try {
      let found = await window.bridge.projectsResolve(gitRemote);
      if (!found)
        found = await window.bridge.projectsUpsert({
          name: projectName,
          git: { url: gitRemote, defaultBranch: "main" },
        });
      setProject(found);
    } catch (reason) {
      setError(String(reason));
    }
  }, [gitRemote, projectName]);
  useEffect(() => {
    void load();
  }, [load]);

  async function readFile(file?: File) {
    if (!file) return;
    setError("");
    try {
      const parsed = parseEnv(await file.text());
      if (!parsed.length)
        throw new Error("No environment variables found in this file.");
      setBusy(true);
      const statuses =
        project && window.bridge
          ? await window.bridge.projectEnvImportReview(project.id, parsed)
          : [];
      setReview(
        parsed.map((entry) => {
          const status =
            statuses.find((item) => item.name === entry.name)?.status || "new";
          return {
            ...entry,
            status,
            secret: isSecretEnvName(entry.name),
            selected: status === "new",
            override: false,
            overrideHost: "local",
          };
        }),
      );
      setFileName(file.name);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function saveImport() {
    if (!project || !review || !window.bridge) return;
    setBusy(true);
    setError("");
    try {
      const imported = review.filter((entry) => entry.selected);
      const env = [...project.env];
      for (const entry of imported) {
        const existing = env.find((item) => item.name === entry.name);
        if (entry.override && existing) {
          existing.hosts = [
            ...new Set([...(existing.hosts || []), entry.overrideHost]),
          ];
          existing.secret ||= entry.secret;
        } else if (existing) {
          existing.secret = entry.secret;
        } else {
          env.push({
            name: entry.name,
            secret: entry.secret,
            availableTo: ["setup", "agent"],
          });
        }
      }
      const updated = await window.bridge.projectsUpsert({ ...project, env });
      for (const entry of imported) {
        if (entry.override)
          await window.bridge.projectHostSecretSet(
            updated.id,
            entry.name,
            entry.overrideHost,
            entry.value,
          );
        else
          await window.bridge.projectSecretSet(
            updated.id,
            entry.name,
            entry.value,
          );
      }
      setProject(await window.bridge.projectsGet(updated.id));
      setReview(null);
      setFileName("");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function updateMetadata(nextEnv: Project["env"]) {
    if (!project || !window.bridge) return;
    const updated = await window.bridge.projectsUpsert({
      ...project,
      env: nextEnv,
    });
    setProject(updated);
  }

  async function addVariable() {
    const name = window.prompt("Variable name");
    if (!name?.trim() || !project) return;
    const value = window.prompt(`Value for ${name}`);
    if (value === null) return;
    const secret = isSecretEnvName(name.trim());
    const updated = await window.bridge!.projectsUpsert({
      ...project,
      env: [
        ...project.env,
        { name: name.trim(), secret, availableTo: ["setup", "agent"] },
      ],
    });
    if (value)
      await window.bridge!.projectSecretSet(updated.id, name.trim(), value);
    setProject(await window.bridge!.projectsGet(updated.id));
  }

  return (
    <section className="project-environment">
      <header className="project-environment-heading">
        <div>
          <h2>Environment</h2>
          <p>
            Variables and secrets every host gets. Secret values stay in secure
            storage.
          </p>
        </div>
        <div>
          <button
            className="secondary"
            disabled={!project}
            onClick={() => fileInput.current?.click()}
          >
            <Upload size={13} /> Import .env
          </button>
          <button className="primary" onClick={() => void addVariable()}>
            <Plus size={14} /> Add variable
          </button>
        </div>
      </header>
      <input
        ref={fileInput}
        type="file"
        accept=".env,text/plain"
        hidden
        onChange={(event) => {
          void readFile(event.target.files?.[0]);
          event.target.value = "";
        }}
      />
      {error && (
        <p role="alert" className="settings-error">
          {error}
        </p>
      )}
      {project ? (
        <>
          <p className="settings-muted">
            {project.env.length} variables ·{" "}
            {project.env.filter((entry) => entry.secret).length} secrets
          </p>
          <div className="workspace-scroll">
            <table className="project-env-table">
              <thead>
                <tr>
                  <th>Key</th>
                  <th>Value</th>
                  <th>Available to</th>
                  <th>Hosts</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {project.env.map((entry) => (
                  <tr key={entry.name}>
                    <td>
                      {entry.secret && <Lock size={12} />}
                      <code>{entry.name}</code>
                    </td>
                    <td>
                      {entry.secret && entry.hasValue ? (
                        <span className="env-masked">
                          {displayedEnvValue("", true, entry.hint)}
                        </span>
                      ) : (
                        <span>{entry.hint || "Not set"}</span>
                      )}
                    </td>
                    <td>
                      <select
                        aria-label={`${entry.name} available to`}
                        value={(entry.availableTo || ["setup", "agent"]).join(
                          ",",
                        )}
                        onChange={(event) =>
                          void updateMetadata(
                            project.env.map((item) =>
                              item.name === entry.name
                                ? {
                                    ...item,
                                    availableTo: event.target.value.split(","),
                                  }
                                : item,
                            ),
                          )
                        }
                      >
                        <option value="setup,agent">setup + agent</option>
                        <option value="agent">agent</option>
                        <option value="setup">setup only</option>
                        <option value="mcp">MCP only</option>
                      </select>
                    </td>
                    <td>
                      {entry.hosts?.length ? (
                        <span className="env-override">
                          Override · {entry.hosts.join(", ")}
                        </span>
                      ) : (
                        "all"
                      )}
                    </td>
                    <td>
                      <button
                        className="icon-button"
                        aria-label={`Remove ${entry.name}`}
                        onClick={() =>
                          void updateMetadata(
                            project.env.filter(
                              (item) => item.name !== entry.name,
                            ),
                          )
                        }
                      >
                        <X size={13} />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div
            className={`project-env-drop${dragging ? " dragging" : ""}`}
            onDragOver={(event) => {
              event.preventDefault();
              setDragging(true);
            }}
            onDragLeave={(event) => {
              if (
                !event.currentTarget.contains(
                  event.relatedTarget as Node | null,
                )
              )
                setDragging(false);
            }}
            onDrop={(event) => {
              event.preventDefault();
              setDragging(false);
              void readFile(event.dataTransfer.files[0]);
            }}
          >
            Drop a .env file here. Keys named *_TOKEN, *_KEY or *_SECRET come in
            as secrets.
          </div>
          <p className="workspace-note">
            Setup only values are available to install steps. MCP only values
            are referenced by MCP servers, not the agent.
          </p>
        </>
      ) : (
        <p className="settings-muted">
          {gitRemote
            ? "Loading project environment…"
            : "Set a git remote to attach this workspace to a project."}
        </p>
      )}
      {review && (
        <div className="project-env-review-backdrop">
          <section
            className="project-env-review"
            role="dialog"
            aria-modal="true"
            aria-labelledby="env-import-title"
          >
            <button
              className="icon-button project-env-review-close"
              aria-label="Close import review"
              onClick={() => setReview(null)}
            >
              <X size={14} />
            </button>
            <h2 id="env-import-title">Import .env</h2>
            <p>
              Review before saving. Nothing is stored until you confirm, and the
              file itself is never kept.
            </p>
            <div className="project-env-review-file">
              <span>
                {fileName} · {review.length} keys
              </span>
              <button
                className="text-button"
                onClick={() => fileInput.current?.click()}
              >
                Choose another file
              </button>
            </div>
            <div className="workspace-scroll">
              <table className="project-env-table">
                <thead>
                  <tr>
                    <th>Key</th>
                    <th>Value</th>
                    <th>Secret</th>
                    <th>Result</th>
                  </tr>
                </thead>
                <tbody>
                  {review.map((entry, index) => (
                    <tr key={entry.name}>
                      <td>
                        <code>{entry.name}</code>
                      </td>
                      <td>{displayedEnvValue(entry.value, entry.secret)}</td>
                      <td>
                        <input
                          type="checkbox"
                          checked={entry.secret}
                          aria-label={`${entry.name} is secret`}
                          onChange={(event) =>
                            setReview(
                              review.map((item, i) =>
                                i === index
                                  ? { ...item, secret: event.target.checked }
                                  : item,
                              ),
                            )
                          }
                        />
                      </td>
                      <td>
                        <label>
                          <input
                            type="checkbox"
                            checked={entry.selected}
                            onChange={(event) =>
                              setReview(
                                review.map((item, i) =>
                                  i === index
                                    ? {
                                        ...item,
                                        selected: event.target.checked,
                                      }
                                    : item,
                                ),
                              )
                            }
                          />
                          <span className={`env-import-status ${entry.status}`}>
                            {entry.status === "same"
                              ? "same value"
                              : entry.status === "exists"
                                ? "exists · keep"
                                : entry.status === "differs"
                                  ? "differs · replace?"
                                  : entry.secret
                                    ? "new secret"
                                    : "new"}
                          </span>
                        </label>
                        {entry.status === "differs" && (
                          <div className="env-override-choice">
                            <label>
                              <input
                                type="checkbox"
                                checked={entry.override}
                                onChange={(event) =>
                                  setReview(
                                    review.map((item, i) =>
                                      i === index
                                        ? {
                                            ...item,
                                            override: event.target.checked,
                                            selected: true,
                                          }
                                        : item,
                                    ),
                                  )
                                }
                              />
                              import as override
                            </label>
                            {entry.override && (
                              <select
                                aria-label={`${entry.name} override host`}
                                value={entry.overrideHost}
                                onChange={(event) =>
                                  setReview(
                                    review.map((item, i) =>
                                      i === index
                                        ? {
                                            ...item,
                                            overrideHost: event.target.value,
                                          }
                                        : item,
                                    ),
                                  )
                                }
                              >
                                <option value="local">This Mac</option>
                                {(project?.targets || [])
                                  .filter((host) => host !== "local")
                                  .map((host) => (
                                    <option key={host} value={host}>
                                      {host}
                                    </option>
                                  ))}
                              </select>
                            )}
                          </div>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="env-import-secret-note">
              <Lock size={14} />
              <div>
                <strong>
                  {review.filter((entry) => entry.secret).length} keys look like
                  secrets, so they are marked as secrets.
                </strong>
                <span>
                  Names ending in _TOKEN, _KEY or _SECRET. Switch one off if it
                  is not a secret; its value is then shown in plain text.
                </span>
              </div>
            </div>
            <footer className="project-env-review-footer">
              <p>
                {review.some((entry) => entry.status === "differs")
                  ? "A variable differs from the saved value. Import it as an override for one host instead?"
                  : "Review the selected keys before importing."}
              </p>
              <button className="secondary" onClick={() => setReview(null)}>
                Cancel
              </button>
              <button
                className="primary"
                disabled={
                  busy || !project || !review.some((entry) => entry.selected)
                }
                onClick={() => void saveImport()}
              >
                {busy
                  ? "Importing…"
                  : `Import ${review.filter((entry) => entry.selected).length} keys`}
              </button>
            </footer>
          </section>
        </div>
      )}
    </section>
  );
}
