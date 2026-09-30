import { useEffect, useMemo, useState } from "react";
import {
  Check,
  FolderOpen,
  GitBranch,
  LoaderCircle,
  Plus,
  X,
} from "lucide-react";
import type { ClaudeAccount, ConnectionProfile } from "./types";
import { isSecretEnvName, parseEnv } from "./projectEnv";

type Source = "git" | "folder" | "empty";
type Step = "source" | "hosts" | "environment" | "creating" | "ready";
type Host = { endpoint: string; label: string; cwd: string; local: boolean };
type Variable = { name: string; value: string; secret: boolean };

function slug(value: string) {
  return (
    value
      .toLowerCase()
      .trim()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project"
  );
}

function decodeFile(file: { base64?: string }) {
  return file.base64 ? atob(file.base64) : "";
}

export function WorkspaceDialog({
  defaultCwd,
  activeEndpoint,
  localSocket,
  connectionProfiles,
  statusByEndpoint,
  onCreate,
  onStart,
  onClose,
}: {
  defaultCwd: string;
  activeEndpoint?: string;
  localSocket: string;
  connectionProfiles: ConnectionProfile[];
  statusByEndpoint: Record<string, string>;
  onCreate(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    endpoint?: string,
  ): Promise<boolean>;
  onStart(): void;
  onClose(): void;
}) {
  const hosts = useMemo<Host[]>(
    () => [
      ...(localSocket
        ? [
            {
              endpoint: localSocket,
              label: "This Mac",
              cwd: defaultCwd,
              local: true,
            },
          ]
        : []),
      ...connectionProfiles
        .filter((profile) => !profile.hidden)
        .map((profile) => ({
          endpoint: `ssh:${profile.id}`,
          label: profile.name || profile.host,
          cwd: `~/sushiai/project`,
          local: false,
        })),
    ],
    [localSocket, connectionProfiles, defaultCwd],
  );
  const [step, setStep] = useState<Step>("source");
  const [source, setSource] = useState<Source>("git");
  const [url, setUrl] = useState("");
  const [cwd, setCwd] = useState(defaultCwd);
  const [folderEndpoint, setFolderEndpoint] = useState(
    activeEndpoint || localSocket,
  );
  const [homeDir, setHomeDir] = useState("");
  const [name, setName] = useState("");
  const [branch, setBranch] = useState("main");
  const [selectedHosts, setSelectedHosts] = useState<string[]>(() =>
    [activeEndpoint || localSocket].filter(Boolean),
  );
  const [variables, setVariables] = useState<Variable[]>([]);
  const [plainCount, setPlainCount] = useState(0);
  const [mcp, setMcp] = useState<Record<string, unknown>>({});
  const [install, setInstall] = useState("");
  const [accountId, setAccountId] = useState("");
  const [accounts, setAccounts] = useState<ClaudeAccount[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<
    Record<string, { state: string; detail: string }>
  >({});
  const localHost = hosts.find((host) => host.local);
  const selected = hosts.filter((host) =>
    selectedHosts.includes(host.endpoint),
  );
  const projectCwd =
    source === "folder"
      ? cwd
      : `${homeDir || localHost?.cwd.replace(/\/[^/]*$/, "") || "~"}/sushiai/${slug(name)}`;

  useEffect(() => {
    window.bridge
      ?.claudeAccountsList()
      .then(setAccounts)
      .catch(() => {});
    if (localSocket)
      window.bridge
        ?.projectInspect(localSocket, { operation: "home" })
        .then((info) => setHomeDir(info.home))
        .catch(() => {});
  }, [localSocket]);

  async function chooseFolder() {
    const host = hosts.find((item) => item.endpoint === folderEndpoint);
    if (!host?.local) return;
    const chosen = await window.bridge?.chooseDirectory();
    if (chosen) setCwd(chosen);
  }

  async function inspectSource() {
    setError("");
    if (source === "git" && !url.trim()) throw new Error("Enter a git URL.");
    if (source === "folder" && !cwd.trim())
      throw new Error("Choose a project folder.");
    if (!name.trim())
      setName(
        source === "folder"
          ? cwd.split("/").filter(Boolean).at(-1) || "New project"
          : source === "git"
            ? url
                .split(/[/:]/)
                .filter(Boolean)
                .at(-1)
                ?.replace(/\.git$/, "") || "New project"
            : "New project",
      );
    if (source === "git") {
      const info = await window.bridge?.projectSourceInspect(url.trim());
      if (info) {
        setBranch(info.branch);
        const entries = parseEnv(info.envExample);
        setVariables(
          entries.map((entry) => ({
            name: entry.name,
            secret: isSecretEnvName(entry.name),
            value: isSecretEnvName(entry.name) ? "" : entry.value,
          })),
        );
        setPlainCount(
          entries.filter((entry) => !isSecretEnvName(entry.name)).length,
        );
        if (info.mcp) {
          try {
            setMcp(JSON.parse(info.mcp).mcpServers || {});
          } catch {
            setMcp({});
          }
        }
        const installs: Record<string, string> = {
          "package-lock.json": "npm ci",
          "pnpm-lock.yaml": "pnpm install --frozen-lockfile",
          "yarn.lock": "yarn install --frozen-lockfile",
          "bun.lock": "bun install --frozen-lockfile",
          "bun.lockb": "bun install --frozen-lockfile",
          "Cargo.lock": "cargo build",
          "uv.lock": "uv sync --locked",
          "poetry.lock": "poetry install",
          "Pipfile.lock": "pipenv sync",
          "Gemfile.lock": "bundle install",
          "composer.lock": "composer install",
          "go.sum": "go mod download",
        };
        if (info.lockFile) setInstall(installs[info.lockFile] || "");
      }
    }
    if (source === "folder") {
      const endpoint = folderEndpoint;
      const info = await window.bridge
        ?.projectInspect(endpoint, { operation: "git_remote", root: cwd })
        .catch(() => null);
      if (info?.remote) setUrl(info.remote);
      for (const file of [
        ".env.example",
        ".mcp.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "Cargo.lock",
        "uv.lock",
        "poetry.lock",
        "Gemfile.lock",
        "go.sum",
      ]) {
        const read = await window.bridge
          ?.projectInspect(endpoint, {
            operation: "read",
            root: cwd,
            path: file,
          })
          .catch(() => null);
        if (!read?.base64) continue;
        if (file === ".env.example") {
          const parsed = parseEnv(decodeFile(read));
          setVariables(
            parsed.map((entry) => ({
              name: entry.name,
              secret: isSecretEnvName(entry.name),
              value: isSecretEnvName(entry.name) ? "" : entry.value,
            })),
          );
          setPlainCount(
            parsed.filter((entry) => !isSecretEnvName(entry.name)).length,
          );
        } else if (file === ".mcp.json") {
          try {
            setMcp(JSON.parse(decodeFile(read)).mcpServers || {});
          } catch {
            setMcp({});
          }
        } else if (file.endsWith("lock") || file.endsWith("lockb")) {
          const commands: Record<string, string> = {
            "package-lock.json": "npm ci",
            "pnpm-lock.yaml": "pnpm install --frozen-lockfile",
            "yarn.lock": "yarn install --frozen-lockfile",
            "bun.lock": "bun install --frozen-lockfile",
            "bun.lockb": "bun install --frozen-lockfile",
            "Cargo.lock": "cargo build",
            "uv.lock": "uv sync --locked",
            "poetry.lock": "poetry install",
            "Gemfile.lock": "bundle install",
            "go.sum": "go mod download",
          };
          setInstall(commands[file] || "");
          break;
        }
      }
    }
    setStep("hosts");
  }

  async function createProject() {
    if (!window.bridge || busy) return;
    setBusy(true);
    setError("");
    setStep("creating");
    const init = Object.fromEntries(
      selected.map((host) => [
        host.endpoint,
        { state: "queued", detail: host.cwd },
      ]),
    );
    setResults(init);
    try {
      const remoteUrl = source === "git" ? url.trim() : url.trim();
      const project = await window.bridge.projectsUpsert({
        name: name.trim() || "New project",
        git: { url: remoteUrl, defaultBranch: branch || "main" },
        env: variables.map(({ name: key, secret }) => ({
          name: key,
          secret,
          availableTo: ["setup", "agent"],
        })),
        mcp,
        setup: { install, check: "" },
        sessions: {
          claudeAccount: accountId || undefined,
          backend: selected.some((host) => !host.local) ? "herdr" : "local",
        },
      });
      for (const item of variables) {
        if (item.value)
          await window.bridge.projectSecretSet(
            project.id,
            item.name,
            item.value,
          );
      }
      const workspaces: { host: Host; cwd: string }[] = [];
      for (const host of selected) {
        setResults((current) => ({
          ...current,
          [host.endpoint]: { state: "working", detail: host.cwd },
        }));
        try {
          const reuseFolder =
            source === "folder" && folderEndpoint === host.endpoint;
          let target = reuseFolder
            ? cwd
            : host.local
              ? projectCwd
              : `~/sushiai/${slug(name)}`;
          if (
            host.local &&
            (source === "git" ||
              source === "empty" ||
              (source === "folder" && !reuseFolder))
          ) {
            if (source === "folder" && !remoteUrl)
              throw new Error(
                "This folder has no git remote to copy to This Mac.",
              );
            await window.bridge.projectLocalCreate({
              url: remoteUrl,
              cwd: target,
              branch,
              empty: source === "empty",
            });
          } else if (!host.local && !reuseFolder) {
            if (source === "folder" && !remoteUrl)
              throw new Error(
                "This folder has no git remote to clone on another host.",
              );
            await window.bridge.projectHostTrust(
              project.id,
              host.endpoint,
              true,
            );
            const prepared = await window.bridge.projectHostPrepare(
              project.id,
              host.endpoint,
            );
            if (!prepared.ok)
              throw new Error(prepared.message || "Host setup failed.");
            target = prepared.path;
          } else if (!host.local && source === "folder" && reuseFolder) {
            await window.bridge.projectHostTrust(
              project.id,
              host.endpoint,
              true,
            );
          }
          workspaces.push({ host, cwd: target });
          setResults((current) => ({
            ...current,
            [host.endpoint]: { state: "ready", detail: target },
          }));
        } catch (reason) {
          setResults((current) => ({
            ...current,
            [host.endpoint]: {
              state: "failed",
              detail: reason instanceof Error ? reason.message : String(reason),
            },
          }));
        }
      }
      if (!workspaces.length)
        throw new Error("No host finished creating the project.");
      const first = workspaces[0];
      const created = await onCreate(
        project.name,
        first.cwd,
        project.sessions.backend || "local",
        "shell",
        first.host.local ? undefined : first.host.endpoint,
      );
      if (!created)
        throw new Error(
          "Project files were created, but the workspace could not open.",
        );
      setStep("ready");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setStep("environment");
    } finally {
      setBusy(false);
    }
  }

  const stepIndex = ["source", "hosts", "environment"].indexOf(step);
  return (
    <div className="new-project-flow">
      <button
        className="new-project-close"
        aria-label="Close"
        onClick={onClose}
      >
        <X size={16} />
      </button>
      <h2>
        {step === "creating"
          ? `Creating ${name || "project"}`
          : step === "ready"
            ? `${name || "Project"} is ready`
            : "New project"}
      </h2>
      {step !== "creating" && step !== "ready" && (
        <div className="new-project-steps">
          {["Source", "Hosts", "Environment"].map((label, index) => (
            <button
              key={label}
              className={
                stepIndex === index
                  ? "active"
                  : stepIndex > index
                    ? "complete"
                    : ""
              }
              onClick={() => {
                if (index < stepIndex)
                  setStep(["source", "hosts", "environment"][index] as Step);
              }}
            >
              <i>{stepIndex > index ? <Check size={12} /> : index + 1}</i>
              {label}
            </button>
          ))}
        </div>
      )}
      {step === "source" && (
        <section className="new-project-pane">
          <div className="new-project-tabs">
            {(
              [
                ["git", "Git repository"],
                ["folder", "Folder on a host"],
                ["empty", "Empty"],
              ] as [Source, string][]
            ).map(([kind, label]) => (
              <button
                key={kind}
                className={source === kind ? "selected" : ""}
                onClick={() => setSource(kind)}
              >
                {label}
              </button>
            ))}
          </div>
          {source === "git" && (
            <label className="new-project-field">
              <GitBranch size={14} />
              <input
                autoFocus
                placeholder="Paste a git URL — git@github.com:... or https://..."
                value={url}
                onChange={(event) => setUrl(event.target.value)}
              />
            </label>
          )}
          {source === "folder" && (
            <label className="new-project-field">
              <span>Host</span>
              <select
                value={folderEndpoint}
                onChange={(event) => {
                  setFolderEndpoint(event.target.value);
                  setSelectedHosts((current) =>
                    current.includes(event.target.value)
                      ? current
                      : [...current, event.target.value],
                  );
                }}
              >
                {hosts.map((host) => (
                  <option key={host.endpoint} value={host.endpoint}>
                    {host.label}
                  </option>
                ))}
              </select>
            </label>
          )}
          {source === "folder" && (
            <label className="new-project-field">
              <FolderOpen size={14} />
              <input
                placeholder="/Users/you/project"
                value={cwd}
                onChange={(event) => setCwd(event.target.value)}
              />
              <button
                onClick={chooseFolder}
                type="button"
                disabled={
                  !hosts.find((host) => host.endpoint === folderEndpoint)?.local
                }
              >
                <FolderOpen size={15} />
              </button>
            </label>
          )}
          {source === "empty" && (
            <p className="new-project-hint">
              Start with an empty git repository at ~/sushiai/
              {slug(name || "project")}.
            </p>
          )}
          {(source === "git" || source === "empty") && (
            <label className="new-project-field">
              <span>Name</span>
              <input
                placeholder="Project name"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
          )}
          {source === "folder" && (
            <label className="new-project-field">
              <span>Name</span>
              <input
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </label>
          )}
          {source === "git" && (
            <label className="new-project-field">
              <span>Branch</span>
              <input
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
              />
            </label>
          )}
          {error && (
            <p className="new-project-error" role="alert">
              {error}
            </p>
          )}
          <footer>
            <span>
              {source === "folder"
                ? "Reads repository settings from this folder"
                : "Reads branch, .env.example, .mcp.json and lock file"}
            </span>
            <button
              className="primary"
              onClick={() =>
                void inspectSource().catch((reason) => setError(String(reason)))
              }
            >
              Continue
            </button>
          </footer>
        </section>
      )}
      {step === "hosts" && (
        <section className="new-project-pane">
          <div className="new-project-hosts">
            {hosts.map((host) => {
              const checked = selectedHosts.includes(host.endpoint);
              return (
                <label key={host.endpoint} className="new-project-host">
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() =>
                      setSelectedHosts((current) =>
                        checked
                          ? current.filter((item) => item !== host.endpoint)
                          : [...current, host.endpoint],
                      )
                    }
                  />
                  <span>{host.label}</span>
                  <small>
                    {host.local
                      ? source === "folder"
                        ? cwd
                        : projectCwd
                      : `~/sushiai/${slug(name || "project")}`}
                  </small>
                  <i>
                    {host.local
                      ? source === "folder"
                        ? "found"
                        : "local"
                      : statusByEndpoint[host.endpoint] === "connected"
                        ? source === "folder"
                          ? "found"
                          : "clones"
                        : "offline"}
                  </i>
                </label>
              );
            })}
          </div>
          {error && (
            <p className="new-project-error" role="alert">
              {error}
            </p>
          )}
          <footer>
            <span>
              Creates on{" "}
              {selected.map((host) => host.label).join(" and ") ||
                "no hosts selected"}
            </span>
            <div>
              <button onClick={() => setStep("source")}>Back</button>
              <button
                className="primary"
                disabled={!selected.length}
                onClick={() => setStep("environment")}
              >
                Continue
              </button>
            </div>
          </footer>
        </section>
      )}
      {step === "environment" && (
        <section className="new-project-pane">
          <label className="new-project-field">
            <span>Name</span>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Project name"
            />
          </label>
          <div className="new-project-caption">SECRETS · FROM .ENV.EXAMPLE</div>
          <div className="new-project-vars">
            {variables
              .map((item, index) => ({ item, index }))
              .filter(({ item }) => item.secret)
              .map(({ item, index }) => (
                <label key={item.name}>
                  <span>♙ {item.name}</span>
                  <input
                    type="password"
                    placeholder="paste value or leave empty"
                    value={item.value}
                    onChange={(event) =>
                      setVariables((current) =>
                        current.map((variable, at) =>
                          at === index
                            ? { ...variable, value: event.target.value }
                            : variable,
                        ),
                      )
                    }
                  />
                </label>
              ))}
            {!variables.some((item) => item.secret) && (
              <p>No secrets found in .env.example</p>
            )}
            <button
              onClick={() =>
                setVariables((current) => [
                  ...current,
                  { name: "", value: "", secret: true },
                ])
              }
            >
              <Plus size={13} /> Add a secret
            </button>
          </div>
          {plainCount > 0 && (
            <p className="new-project-hint">
              {plainCount} plain variables are filled from .env.example · show
            </p>
          )}
          <div className="new-project-caption">RUNS WITH</div>
          <label className="new-project-field">
            <span>MCP servers</span>
            <input value={Object.keys(mcp).join(", ") || "None"} readOnly />
          </label>
          <label className="new-project-field">
            <span>Install</span>
            <input
              value={install}
              placeholder="Install command (optional)"
              onChange={(event) => setInstall(event.target.value)}
            />
          </label>
          <label className="new-project-field">
            <span>Claude Code</span>
            <select
              value={accountId}
              onChange={(event) => setAccountId(event.target.value)}
            >
              <option value="">Default for new sessions</option>
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.label}
                </option>
              ))}
            </select>
          </label>
          {error && (
            <p className="new-project-error" role="alert">
              {error}
            </p>
          )}
          <footer>
            <span>
              {variables.filter((item) => !item.value).length} secrets empty —
              fill them now or later
            </span>
            <div>
              <button onClick={() => setStep("hosts")}>Back</button>
              <button
                className="primary"
                disabled={busy}
                onClick={() => void createProject()}
              >
                {busy ? "Creating…" : "Create project"}
              </button>
            </div>
          </footer>
        </section>
      )}
      {step === "creating" && (
        <section className="new-project-pane">
          <p className="new-project-hint">
            Continues in the background if you close this.
          </p>
          {selected.map((host) => {
            const result = results[host.endpoint];
            return (
              <div className="new-project-progress" key={host.endpoint}>
                <strong>
                  {result?.state === "working" ? (
                    <LoaderCircle size={14} className="spinning" />
                  ) : result?.state === "ready" ? (
                    <Check size={14} />
                  ) : (
                    <i />
                  )}
                  {host.label}
                </strong>
                <span>{result?.detail || host.cwd}</span>
                <small>{result?.state || "queued"}</small>
              </div>
            );
          })}
          <footer>
            <span>Setup continues when you close this window</span>
            <button onClick={onClose}>Close</button>
          </footer>
        </section>
      )}
      {step === "ready" && (
        <section className="new-project-pane">
          <div className="new-project-summary">
            {selected.map((host) => (
              <div className="new-project-progress" key={host.endpoint}>
                <strong>{host.label}</strong>
                <span>{results[host.endpoint]?.detail}</span>
                <small>{results[host.endpoint]?.state}</small>
              </div>
            ))}
            <div className="new-project-progress">
              <strong>Environment</strong>
              <span>
                {variables.length} variables · {Object.keys(mcp).length} MCP
                servers
              </span>
            </div>
            <div className="new-project-progress">
              <strong>Claude Code</strong>
              <span>
                {accounts.find((account) => account.id === accountId)?.label ||
                  "Default"}
              </span>
            </div>
          </div>
          <footer>
            <span>Change any of this later in Project settings</span>
            <div>
              <button onClick={onClose}>Done</button>
              <button className="primary" onClick={onStart}>
                Start a session
              </button>
            </div>
          </footer>
        </section>
      )}
    </div>
  );
}
