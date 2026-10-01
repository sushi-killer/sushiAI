import { importChanges } from "./projectEnvImport.ts";
import { useCallback, useEffect, useRef, useState } from "react";
import { Lock, Plus, X } from "lucide-react";
import type { ConnectionProfile, Project } from "./types";
import { Banner, Tag, Toggle } from "./orchestrator/ui";
import type { Tone } from "./orchestrator/helpers";
import { ProjectPage } from "./ProjectPage";

type Status = "new" | "exists" | "same" | "differs";
type Choice = "import" | "override" | "skip";
type ReviewEntry = {
  name: string;
  value: string;
  secret: boolean;
  status: Status;
  choice: Choice;
};
type Env = Project["env"][number];

const AVAILABLE: Record<string, string> = {
  "setup,agent": "setup + agent",
  agent: "agent",
  setup: "setup only",
  mcp: "MCP only",
};
const DEFAULT_AVAILABLE = "setup,agent";

/** `••••••••••••  …q7Xa`: the dots stand for the hidden value and the tail is
 * the hint the secure storage kept. */
/** Dots for a hidden value, then its last characters set apart. */
function Masked({ hint, dots }: { hint: string | undefined; dots: number }) {
  const tail = (hint || "").replace(/^[•…\s]+/, "");
  return (
    <>
      {"•".repeat(dots)}
      {tail && <span className="pd-mask-tail">{`…${tail}`}</span>}
    </>
  );
}

function defaultChoice(status: Status): Choice {
  return status === "new" || status === "differs" ? "import" : "skip";
}

/** What the result tag of one review row says, and how it is toned. */
function resultOf(
  entry: ReviewEntry,
  hostName: string,
): { label: string; tone: Tone } {
  if (entry.choice === "override")
    return { label: `override · ${hostName}`, tone: "info" };
  if (entry.status === "new")
    return entry.choice === "import"
      ? { label: entry.secret ? "new secret" : "new", tone: "ok" }
      : { label: "skipped", tone: "neutral" };
  if (entry.status === "differs")
    return entry.choice === "import"
      ? { label: "differs · replace?", tone: "warning" }
      : { label: "differs · keep", tone: "neutral" };
  if (entry.choice === "import")
    return { label: "will replace", tone: "warning" };
  return {
    label: entry.status === "same" ? "same value" : "exists · keep",
    tone: "neutral",
  };
}

export function ProjectEnvironmentTab({
  project,
  setProject,
  gitRemote,
  projectName,
  cwd,
  remote = false,
  targets = [],
}: {
  /** The dialog owns the project: every tab reads and replaces this one. */
  project: Project | null;
  setProject(project: Project | null): void;
  gitRemote: string;
  projectName: string;
  cwd?: string;
  remote?: boolean;
  targets?: ConnectionProfile[];
}) {
  const [error, setError] = useState("");
  const [review, setReview] = useState<ReviewEntry[] | null>(null);
  const [fileName, setFileName] = useState("");
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [adding, setAdding] = useState<{ name: string; value: string } | null>(
    null,
  );
  const [addingSecret, setAddingSecret] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const addingName = adding?.name.trim() ?? "";
  // The main process decides what looks like a secret; this only asks.
  useEffect(() => {
    if (!addingName || !window.bridge) return setAddingSecret(false);
    let live = true;
    void window.bridge
      .projectEnvClassify([addingName])
      .then(([secret]) => live && setAddingSecret(!!secret));
    return () => {
      live = false;
    };
  }, [addingName]);
  const autoImported = useRef(false);

  const load = useCallback(async () => {
    if (!window.bridge || !gitRemote || project) return;
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
  }, [gitRemote, projectName, project, setProject]);
  useEffect(() => {
    void load();
  }, [load]);

  // First open of a project with an empty Environment: bring in the local
  // checkout's .env files once. Nothing exists yet, so nothing can conflict;
  // a later import still goes through the review dialog.
  useEffect(() => {
    if (!project || !cwd || remote || !window.bridge || autoImported.current)
      return;
    autoImported.current = true;
    const flag = `sushiai.autoImport.env.${project.id}`;
    if (project.env.length || localStorage.getItem(flag)) return;
    const bridge = window.bridge;
    void (async () => {
      try {
        localStorage.setItem(flag, "1");
        const result = await bridge.projectImportLocal(project.id, cwd);
        setProject(result.project);
      } catch (reason) {
        setError(String(reason));
      }
    })();
  }, [project, cwd, remote, setProject]);

  const hostName = (key: string) =>
    key === "local"
      ? "This Mac"
      : targets.find((host) => `ssh:${host.id}` === key)?.name ||
        key.replace(/^ssh:/, "");
  const firstHost = targets.find((host) => !host.hidden);
  const overrideHost =
    (project?.targets || []).find((host) => host !== "local") ||
    (firstHost ? `ssh:${firstHost.id}` : "local");

  async function readFile(file?: File) {
    if (!file) return;
    setError("");
    try {
      if (!project || !window.bridge) return;
      setBusy(true);
      const reviewed = await window.bridge.projectEnvReviewText(
        project.id,
        await file.text(),
      );
      if (!reviewed.length)
        throw new Error("No environment variables found in this file.");
      setReview(
        reviewed.map((entry) => ({
          ...entry,
          choice: defaultChoice(entry.status as Status),
        })),
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
      const imported = review.filter((entry) => entry.choice !== "skip");
      // Start from what is stored now, not from this tab's copy.
      const fresh = (await window.bridge.projectsGet(project.id)) ?? project;
      const set = importChanges(fresh.env, imported, overrideHost);
      const updated = await window.bridge.projectEnvUpdate(fresh.id, { set });
      for (const entry of imported) {
        if (entry.choice === "override")
          await window.bridge.projectHostSecretSet(
            updated.id,
            entry.name,
            overrideHost,
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

  async function updateEnv(change: {
    set?: Project["env"];
    remove?: string[];
  }) {
    if (!project || !window.bridge) return;
    setProject(await window.bridge.projectEnvUpdate(project.id, change));
  }

  async function addVariable() {
    if (!adding || !project || !window.bridge) return;
    const name = adding.name.trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      setError("A variable name is letters, digits and underscores.");
      return;
    }
    if (project.env.some((entry) => entry.name === name)) {
      setError(`${name} already exists.`);
      return;
    }
    setError("");
    try {
      const [secret] = await window.bridge.projectEnvClassify([name]);
      const updated = await window.bridge.projectEnvUpdate(project.id, {
        set: [{ name, secret, availableTo: ["setup", "agent"] }],
      });
      if (adding.value)
        await window.bridge.projectSecretSet(updated.id, name, adding.value);
      setProject(await window.bridge.projectsGet(updated.id));
      setAdding(null);
    } catch (reason) {
      setError(String(reason));
    }
  }

  const fileInputEl = (
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
  );

  if (review)
    return (
      <ProjectPage
        title="Import .env"
        subtitle="Review before saving. Nothing is stored until you confirm, and the file itself is never kept."
      >
        {fileInputEl}
        <div className="pd-toolbar">
          <span>
            {fileName} · {review.length} keys
          </span>
          <button
            className="ui-button ghost"
            onClick={() => fileInput.current?.click()}
          >
            Choose another file
          </button>
        </div>
        {error && (
          <p role="alert" className="pd-alert">
            {error}
          </p>
        )}
        <div
          className="pd-table pd-import"
          role="table"
          aria-label="Import review"
        >
          <div className="pd-row head" role="row">
            <span role="columnheader">Key</span>
            <span role="columnheader">Value</span>
            <span role="columnheader">Secret</span>
            <span role="columnheader">Result</span>
          </div>
          {review.map((entry, index) => {
            const result = resultOf(entry, hostName(overrideHost));
            const patch = (next: Partial<ReviewEntry>) =>
              setReview(
                review.map((item, at) =>
                  at === index ? { ...item, ...next } : item,
                ),
              );
            const cycle: Choice =
              entry.status === "differs" && entry.choice === "import"
                ? overrideHost === "local"
                  ? "skip"
                  : "override"
                : entry.choice === "override"
                  ? "skip"
                  : entry.choice === "skip"
                    ? "import"
                    : "skip";
            return (
              <div className="pd-row" role="row" key={entry.name}>
                <span className="pd-cell pd-mono" role="cell">
                  {entry.name}
                </span>
                <span
                  className={`pd-cell pd-mono ${entry.secret ? "pd-faint" : "pd-muted"}`}
                  role="cell"
                >
                  {entry.secret ? (
                    <Masked hint={entry.value.slice(-4)} dots={8} />
                  ) : (
                    entry.value
                  )}
                </span>
                <span className="pd-flex" role="cell">
                  <Toggle
                    checked={entry.secret}
                    label={`${entry.name} is secret`}
                    onChange={(secret) => patch({ secret })}
                  />
                </span>
                <span className="pd-flex" role="cell">
                  <button
                    type="button"
                    aria-label={`${entry.name}: ${result.label}`}
                    title="Click to change what happens to this key"
                    onClick={() => patch({ choice: cycle })}
                  >
                    <Tag tone={result.tone}>{result.label}</Tag>
                  </button>
                </span>
              </div>
            );
          })}
        </div>
        <Banner
          tone="info"
          title={`${review.filter((entry) => entry.secret).length} keys look like secrets, so they are marked as secrets.`}
          body="Names that look like tokens, keys, passwords or DSNs. Switch one off if it is not a secret; its value is then shown in plain text."
        />
        <div className="pd-import-actions">
          <p>
            {review.find((entry) => entry.status === "differs")
              ? `${review.find((entry) => entry.status === "differs")!.name} differs from the saved value. Import it as an override for one host instead?`
              : "Review the selected keys before importing."}
          </p>
          <button
            className="ui-button ghost"
            onClick={() => {
              setReview(null);
              setFileName("");
            }}
          >
            Cancel
          </button>
          <button
            className="ui-button primary"
            disabled={
              busy ||
              !project ||
              !review.some((entry) => entry.choice !== "skip")
            }
            onClick={() => void saveImport()}
          >
            {busy
              ? "Importing…"
              : `Import ${review.filter((entry) => entry.choice !== "skip").length} keys`}
          </button>
        </div>
      </ProjectPage>
    );

  const keychain = /Mac/i.test(navigator.platform || navigator.userAgent)
    ? "the macOS Keychain"
    : "secure storage";
  return (
    <ProjectPage
      title="Environment"
      subtitle={`Variables and secrets every host gets. Secret values stay in ${keychain}.`}
    >
      {fileInputEl}
      <div className="pd-toolbar">
        <span>
          {project
            ? `${project.env.length} variables · ${project.env.filter((entry) => entry.secret).length} secrets`
            : ""}
        </span>
        <button
          className="ui-button secondary"
          disabled={!project}
          onClick={() => fileInput.current?.click()}
        >
          Import .env
        </button>
        <button
          className="ui-button primary"
          disabled={!project}
          onClick={() => setAdding({ name: "", value: "" })}
        >
          <Plus size={14} aria-hidden /> Add variable
        </button>
      </div>
      {error && (
        <p role="alert" className="pd-alert">
          {error}
        </p>
      )}
      {project ? (
        <>
          <div className="pd-table pd-env" role="table" aria-label="Variables">
            <div className="pd-row head" role="row">
              <span />
              <span role="columnheader">Key</span>
              <span role="columnheader">Value</span>
              <span role="columnheader">Available to</span>
              <span role="columnheader">Hosts</span>
            </div>
            {adding && (
              <form
                className="pd-row pd-adding"
                role="row"
                onSubmit={(event) => {
                  event.preventDefault();
                  void addVariable();
                }}
              >
                <span />
                <input
                  className="pd-input pd-mono"
                  aria-label="Variable name"
                  placeholder="NAME"
                  autoFocus
                  value={adding.name}
                  onChange={(event) =>
                    setAdding({ ...adding, name: event.target.value })
                  }
                />
                <input
                  className="pd-input pd-mono"
                  aria-label="Variable value"
                  type={addingSecret ? "password" : "text"}
                  placeholder="value"
                  value={adding.value}
                  onChange={(event) =>
                    setAdding({ ...adding, value: event.target.value })
                  }
                />
                <button className="ui-button primary" type="submit">
                  Add
                </button>
                <button
                  className="ui-button ghost"
                  type="button"
                  onClick={() => setAdding(null)}
                >
                  Cancel
                </button>
              </form>
            )}
            {project.env.map((entry) => (
              <EnvRow
                key={entry.name}
                entry={entry}
                hostName={hostName}
                overrideValue={
                  (entry.hosts ?? [])
                    .map(
                      (host) => project.hosts?.[host]?.overrides?.[entry.name],
                    )
                    .find((value) => value !== undefined) as string | undefined
                }
                onAvailable={(value) =>
                  void updateEnv({
                    set: [{ ...entry, availableTo: value.split(",") }],
                  })
                }
                onRemove={() => void updateEnv({ remove: [entry.name] })}
              />
            ))}
            {!project.env.length && !adding && (
              <p className="pd-empty pd-row">
                No variables yet. Add one, or drop a .env file below.
              </p>
            )}
          </div>
          <div
            className={`pd-drop${dragging ? " dragging" : ""}`}
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
            Drop a .env file here. Names that look like tokens, keys, passwords
            or DSNs come in as secrets.
          </div>
          <p className="pd-note">
            {
              "Setup only — the install step can use it; the agent never receives it (private registry). MCP only — reaches an MCP server through ${VAR}; never in the agent’s environment."
            }
          </p>
        </>
      ) : (
        <p className="pd-empty">
          {gitRemote
            ? "Loading project environment…"
            : "Set a git remote to attach this workspace to a project."}
        </p>
      )}
    </ProjectPage>
  );
}

function EnvRow({
  entry,
  hostName,
  overrideValue,
  onAvailable,
  onRemove,
}: {
  entry: Env;
  hostName(key: string): string;
  /** The value this variable has on its host when it has no base value. */
  overrideValue?: string;
  onAvailable(value: string): void;
  onRemove(): void;
}) {
  const availableTo = (entry.availableTo || ["setup", "agent"]).join(",");
  const special = availableTo === "setup" || availableTo === "mcp";
  const label = AVAILABLE[availableTo] || availableTo.replace(",", " + ");
  const overrides = entry.hosts || [];
  return (
    <div className="pd-row" role="row">
      <span className="pd-cell lock" role="cell">
        {entry.secret && <Lock size={13} aria-hidden />}
      </span>
      <span className="pd-cell pd-mono key" role="cell">
        {entry.name}
      </span>
      <span
        className={`pd-cell pd-mono ${entry.secret ? "pd-faint" : "pd-muted"}`}
        role="cell"
      >
        {entry.secret ? (
          entry.hasValue ? (
            <Masked hint={entry.hint} dots={12} />
          ) : (
            "Not set"
          )
        ) : entry.hasValue ? (
          entry.hint || "Set"
        ) : overrides.length ? (
          (overrideValue ?? `${hostName(overrides[0])} override`)
        ) : (
          "Not set"
        )}
      </span>
      <span className="pd-cell" role="cell">
        <span className="pd-avail">
          {special ? (
            <Tag tone="info" dot={false}>
              {label}
            </Tag>
          ) : (
            label
          )}
          <select
            aria-label={`${entry.name} available to`}
            value={AVAILABLE[availableTo] ? availableTo : DEFAULT_AVAILABLE}
            onChange={(event) => onAvailable(event.target.value)}
          >
            <option value="setup,agent">setup + agent</option>
            <option value="agent">agent</option>
            <option value="setup">setup only</option>
            <option value="mcp">MCP only</option>
          </select>
        </span>
      </span>
      <span className="pd-cell pd-faint" role="cell">
        {overrides.length ? (
          <span title={overrides.map(hostName).join(", ")}>
            <Tag tone="warning" dot={false}>
              {entry.hasValue
                ? overrides.length === 1
                  ? "1 override"
                  : `${overrides.length} overrides`
                : // No base value: the variable exists only on these hosts.
                  overrides.length === 1
                  ? `${hostName(overrides[0])} only`
                  : `${overrides.length} hosts only`}
            </Tag>
          </span>
        ) : (
          "all"
        )}
      </span>
      <button
        className="pd-row-remove"
        aria-label={`Remove ${entry.name}`}
        onClick={onRemove}
      >
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}
