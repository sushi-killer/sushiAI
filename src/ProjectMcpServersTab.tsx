import { useCallback, useEffect, useMemo, useState } from "react";
import { Plus, Upload } from "lucide-react";
import type {
  ClaudeMcpServer,
  ClaudePlugin,
  Project,
  SkillCatalogItem,
} from "./types";
import {
  groupAlsoInProject,
  importedMcpServers,
  mcpVariableReferences,
  unknownMcpVariables,
  type ProjectMcpServer,
} from "./projectMcp";

type Props = {
  project: Project;
  cwd: string;
  endpoint?: string;
  remote: boolean;
  onChange: (project: Project) => Promise<void>;
};

function serversOf(project: Project): Record<string, ProjectMcpServer> {
  const mcp = project.mcp as { mcpServers?: unknown };
  return mcp.mcpServers &&
    typeof mcp.mcpServers === "object" &&
    !Array.isArray(mcp.mcpServers)
    ? (mcp.mcpServers as Record<string, ProjectMcpServer>)
    : {};
}

function usageFor(
  plugin: ClaudePlugin,
  skills: SkillCatalogItem[],
  cwd: string,
) {
  const own = skills.filter((skill) => skill.plugin === plugin.name);
  return own.reduce(
    (sum, skill) =>
      sum +
      (skill.recentUses || []).filter(
        (use) => use.project === cwd && use.at > Date.now() - 30 * 86400000,
      ).length,
    0,
  );
}

export function ProjectMcpServersTab({
  project,
  cwd,
  endpoint,
  remote,
  onChange,
}: Props) {
  const [servers, setServers] = useState<ClaudeMcpServer[]>([]);
  const [plugins, setPlugins] = useState<ClaudePlugin[]>([]);
  const [skills, setSkills] = useState<SkillCatalogItem[]>([]);
  const [name, setName] = useState("");
  const [definition, setDefinition] = useState("");
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const ownServers = useMemo(() => serversOf(project), [project]);
  const unknown = useMemo(
    () => unknownMcpVariables(ownServers, project.env),
    [ownServers, project.env],
  );
  const secretReferences = mcpVariableReferences(ownServers).filter((name) =>
    project.env.some((entry) => entry.name === name && entry.secret),
  ).length;

  const load = useCallback(async () => {
    if (!window.bridge) return;
    const results = await Promise.allSettled([
      window.bridge.claudeMcpList(cwd, endpoint),
      window.bridge.claudePluginsList(cwd, endpoint),
      remote
        ? Promise.resolve([] as SkillCatalogItem[])
        : window.bridge.catalog("skills", { force: true }),
    ]);
    if (results[0].status === "fulfilled") setServers(results[0].value.servers);
    if (results[1].status === "fulfilled") setPlugins(results[1].value.plugins);
    if (results[2].status === "fulfilled") setSkills(results[2].value);
    const failure = results.find((result) => result.status === "rejected");
    setError(failure?.status === "rejected" ? String(failure.reason) : "");
  }, [cwd, endpoint, remote]);

  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: Record<string, ProjectMcpServer>) {
    setBusy(true);
    setError("");
    try {
      await onChange({ ...project, mcp: { ...project.mcp, mcpServers: next } });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function setDisabledServers(disabledMcpServers: string[]) {
    setBusy(true);
    setError("");
    try {
      await onChange({
        ...project,
        mcp: { ...project.mcp, disabledMcpServers },
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function importFile(file?: File) {
    if (!file) return;
    try {
      const parsed = importedMcpServers(JSON.parse(await file.text()));
      await save({ ...ownServers, ...parsed });
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : "Could not import .mcp.json.",
      );
    }
  }

  async function addServer() {
    try {
      const value = JSON.parse(definition) as ProjectMcpServer;
      if (!name.trim()) throw new Error("Enter a server name.");
      if (!value || typeof value !== "object" || Array.isArray(value))
        throw new Error("A server definition must be a JSON object.");
      await save({ ...ownServers, [name.trim()]: value });
      setName("");
      setDefinition("");
      setAdding(false);
    } catch (reason) {
      setError(
        reason instanceof Error ? reason.message : "Invalid server definition.",
      );
    }
  }

  async function toggleServer(
    server: ClaudeMcpServer | (ClaudePlugin & { kind: "Plugin" }),
  ) {
    if (!window.bridge || busy) return;
    setBusy(true);
    setError("");
    try {
      if ("kind" in server) {
        const result = await window.bridge.claudePluginsToggle({
          cwd,
          endpoint,
          name: server.name,
          disabled: !server.disabled,
        });
        setPlugins(result.plugins);
      } else {
        const result = await window.bridge.claudeMcpToggle({
          cwd,
          endpoint,
          name: server.name,
          source: server.source,
          disabled: !server.disabled,
        });
        setServers(result.servers);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  const also = groupAlsoInProject(servers, plugins);
  const repoServers = also.repo;
  const otherServers = also.personal;
  return (
    <section className="project-mcp-tab">
      <header className="project-mcp-heading">
        <div>
          <h2>MCP servers</h2>
          <p>
            Every run on every host gets these servers. Tokens are ${"${VAR}"}{" "}
            references to Environment.
          </p>
        </div>
        <div className="project-mcp-actions">
          <label className="secondary project-mcp-import">
            <Upload size={13} /> Import .mcp.json
            <input
              type="file"
              accept=".json,application/json"
              onChange={(event) => void importFile(event.target.files?.[0])}
            />
          </label>
          <button className="primary" onClick={() => setAdding(true)}>
            <Plus size={14} /> Add server
          </button>
        </div>
      </header>
      <p className="project-mcp-count">
        {Object.keys(ownServers).length} servers · {secretReferences} use
        {secretReferences === 1 ? "s" : " secrets"}
      </p>
      {error ? (
        <p role="alert" className="settings-error">
          {error}
        </p>
      ) : null}
      {unknown.length ? (
        <p role="alert" className="project-mcp-error">
          Unknown Environment variable{unknown.length === 1 ? "" : "s"}:{" "}
          {unknown.map((key) => "${" + key + "}").join(", ")}
        </p>
      ) : null}
      <div className="project-mcp-list">
        {Object.entries(ownServers).map(([serverName, server]) => {
          const invalid = unknownMcpVariables(server, project.env);
          const disabled = (
            (project.mcp as { disabledMcpServers?: string[] })
              .disabledMcpServers || []
          ).includes(serverName);
          return (
            <article className="project-mcp-card" key={serverName}>
              <div className="project-mcp-icon">
                {serverName.slice(0, 2).toUpperCase()}
              </div>
              <div className="project-mcp-info">
                <strong>{serverName}</strong>
                <span>
                  {server.url ||
                    [server.command, ...(server.args || [])]
                      .filter(Boolean)
                      .join(" ")}
                </span>
                {invalid.length ? (
                  <small role="alert">
                    Unknown: {invalid.map((key) => "${" + key + "}").join(", ")}
                  </small>
                ) : null}
              </div>
              <input
                type="checkbox"
                aria-label={`${serverName} server`}
                checked={!disabled}
                disabled={busy}
                onChange={() => {
                  const next = new Set(
                    (project.mcp as { disabledMcpServers?: string[] })
                      .disabledMcpServers || [],
                  );
                  if (disabled) next.delete(serverName);
                  else next.add(serverName);
                  void setDisabledServers([...next].sort());
                }}
              />
            </article>
          );
        })}
        {!Object.keys(ownServers).length ? (
          <div className="workspace-control-empty">
            No project MCP servers yet.
          </div>
        ) : null}
      </div>
      {adding ? (
        <div className="project-mcp-editor">
          <input
            aria-label="Server name"
            placeholder="Server name"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <textarea
            aria-label="Server definition"
            placeholder={'{"command":"npx","args":[]}'}
            value={definition}
            onChange={(event) => setDefinition(event.target.value)}
          />
          <div>
            <button className="secondary" onClick={() => setAdding(false)}>
              Cancel
            </button>
            <button
              className="primary"
              disabled={busy}
              onClick={() => void addServer()}
            >
              Save server
            </button>
          </div>
        </div>
      ) : null}
      <h3 className="project-mcp-subheading">Also in this project</h3>
      <div className="project-mcp-list project-mcp-list-secondary">
        {repoServers.map((server) => (
          <article className="project-mcp-card" key={`repo:${server.name}`}>
            <div className="project-mcp-info">
              <strong>
                {server.name}
                <small>.mcp.json</small>
              </strong>
              <span>Every host gets it through the repo.</span>
            </div>
            <span className="project-mcp-badge">● from repo</span>
          </article>
        ))}
        {otherServers.map((server) => (
          <article className="project-mcp-card" key={`server:${server.name}`}>
            <div className="project-mcp-info">
              <strong>
                {server.name}
                <small>~/.claude.json</small>
              </strong>
              <span>Your Claude config on this Mac</span>
            </div>
            <input
              type="checkbox"
              aria-label={`${server.name} server`}
              checked={!server.disabled}
              disabled={busy}
              onChange={() => void toggleServer(server)}
            />
          </article>
        ))}
        {also.plugins.map((plugin) => (
          <article className="project-mcp-card" key={`plugin:${plugin.name}`}>
            <div className="project-mcp-info">
              <strong>
                {plugin.name}
                <small>plugin</small>
              </strong>
              <span>
                Claude plugin · {usageFor(plugin, skills, cwd)} uses in 30 days
              </span>
            </div>
            <input
              type="checkbox"
              aria-label={`${plugin.name} plugin`}
              checked={!plugin.disabled}
              disabled={busy}
              onChange={() => void toggleServer({ ...plugin, kind: "Plugin" })}
            />
          </article>
        ))}
        {remote ? (
          <div className="workspace-control-empty">
            Claude plugins and personal settings are shown on this Mac.
          </div>
        ) : null}
      </div>
    </section>
  );
}
