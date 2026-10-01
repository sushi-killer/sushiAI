import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Plus } from "lucide-react";
import type {
  ClaudeMcpServer,
  ClaudePlugin,
  ConnectionProfile,
  Project,
  SkillCatalogItem,
} from "./types";
import { Tag, Toggle } from "./orchestrator/ui";
import { ProjectPage } from "./ProjectPage";
import { FolderImport, importSummary } from "./FolderImport";
import {
  claudeToolName,
  mcpCountLine,
  serversUsingSecrets,
  groupAlsoInProject,
  pluginLine,
  usesText,
  unknownMcpVariables,
  type ProjectMcpServer,
} from "./projectMcp";

type Props = {
  project: Project;
  cwd: string;
  endpoint?: string;
  remote: boolean;
  /** SSH hosts to ask which servers cannot start there. */
  hosts?: ConnectionProfile[];
  /** Replaces the dialog's project with a fresher one (after an import). */
  onProject: (project: Project) => void;
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

const MARKS: Record<string, string> = { github: "GH", postgres: "PG" };
function markOf(name: string): string {
  return MARKS[name.toLowerCase()] || name.slice(0, 1).toUpperCase();
}

/** `npx server-github · KEY=${VAR}` or `https://… · Authorization: Bearer
 * ${VAR}`: what the server runs or reaches, then the variables it is given. */
function serverLine(server: ProjectMcpServer): string {
  const run =
    server.url ||
    [server.command, ...(server.args || [])].filter(Boolean).join(" ");
  const variables = Object.entries(server.env || {}).map(
    ([key, value]) => `${key}=${value}`,
  );
  const headers = Object.entries(server.headers || {}).map(
    ([key, value]) => `${key}: ${value}`,
  );
  return [run, ...variables, ...headers].filter(Boolean).join(" · ");
}

/** The text with every `${VAR}` reference in the accent colour. */
function Highlighted({ text }: { text: string }) {
  return (
    <>
      {text
        .split(/(\$\{[A-Za-z_][A-Za-z0-9_]*\})/)
        .map((part, index) =>
          part.startsWith("${") ? <b key={index}>{part}</b> : part,
        )}
    </>
  );
}

export function ProjectMcpServersTab({
  project,
  cwd,
  endpoint,
  remote,
  hosts = [],
  onProject,
}: Props) {
  const [servers, setServers] = useState<ClaudeMcpServer[]>([]);
  const [plugins, setPlugins] = useState<ClaudePlugin[]>([]);
  const [skills, setSkills] = useState<SkillCatalogItem[]>([]);
  const [usage, setUsage] = useState<{
    servers: Record<string, number>;
    plugins: Record<string, { uses: number; servers: string[] }>;
  }>({ servers: {}, plugins: {} });
  const [name, setName] = useState("");
  const [definition, setDefinition] = useState("");
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  // Server name -> what a host lacks to run it, from each host's own check.
  const [needs, setNeeds] = useState<Record<string, string[]>>({});
  useEffect(() => {
    let cancelled = false;
    // Without a remote no host can hold this project: nothing to check.
    if (!project.git?.url) {
      setNeeds({});
      return;
    }
    void Promise.all(
      hosts.map(async (host) => {
        try {
          const check = await window.bridge?.projectHostCheck(
            project.id,
            `ssh:${host.id}`,
          );
          return (check?.mcp.missing ?? []).map(
            (item) =>
              [item.name, `needs ${item.command} on ${host.name}`] as const,
          );
        } catch {
          return [];
        }
      }),
    ).then((found) => {
      if (cancelled) return;
      const next: Record<string, string[]> = {};
      for (const [serverName, text] of found.flat())
        (next[serverName] ||= []).push(text);
      setNeeds(next);
    });
    return () => {
      cancelled = true;
    };
  }, [hosts, project.id, project.mcp]);
  const importInput = useRef<HTMLInputElement>(null);
  const ownServers = useMemo(() => serversOf(project), [project]);
  const unknown = useMemo(
    () => unknownMcpVariables(ownServers, project.env),
    [ownServers, project.env],
  );
  // Servers that reference at least one secret (not the secrets themselves).
  const secretReferences = serversUsingSecrets(ownServers, project.env);
  const hasRemote = !!project.git?.url;

  const load = useCallback(async () => {
    if (!window.bridge) return;
    const results = await Promise.allSettled([
      window.bridge.claudeMcpList(cwd, endpoint),
      window.bridge.claudePluginsList(cwd, endpoint),
      remote
        ? Promise.resolve([] as SkillCatalogItem[])
        : window.bridge.catalog("skills", { force: true }),
      remote
        ? Promise.resolve({ servers: {}, plugins: {} })
        : window.bridge.claudeMcpUsage(cwd),
    ]);
    if (results[0].status === "fulfilled") setServers(results[0].value.servers);
    if (results[1].status === "fulfilled") setPlugins(results[1].value.plugins);
    if (results[2].status === "fulfilled") setSkills(results[2].value);
    if (results[3].status === "fulfilled") setUsage(results[3].value);
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
      const set = Object.fromEntries(
        Object.entries(next).filter(
          ([name, server]) =>
            JSON.stringify(ownServers[name]) !== JSON.stringify(server),
        ),
      );
      const remove = Object.keys(ownServers).filter((name) => !(name in next));
      onProject(
        await window.bridge!.projectMcpUpdate(project.id, { set, remove }),
      );
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
      onProject(
        await window.bridge!.projectMcpUpdate(project.id, {
          disabled: disabledMcpServers,
        }),
      );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function importFile(file?: File) {
    if (!file || !window.bridge) return;
    try {
      const result = await window.bridge.projectImportMcpText(
        project.id,
        await file.text(),
      );
      onProject(result.project);
      setNotice(importSummary(result));
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
  // A server the project imported from the repo's .mcp.json is its own now:
  // it is not listed a second time as coming from the repo.
  const repoServers = also.repo.filter((item) => !(item.name in ownServers));
  const otherServers = also.personal;
  const disabledNames =
    (project.mcp as { disabledMcpServers?: string[] }).disabledMcpServers || [];
  return (
    <ProjectPage
      title="MCP servers"
      subtitle={
        hasRemote
          ? "Every run on every host gets these servers. Tokens are ${VAR} references to Environment."
          : "Every run on this Mac gets these servers. Tokens are ${VAR} references to Environment."
      }
    >
      <FolderImport
        project={project}
        cwd={cwd}
        endpoint={endpoint}
        onProject={onProject}
      />
      {notice && (
        <p role="status" className="pd-folder-import">
          {notice}
        </p>
      )}
      <div className="pd-toolbar">
        <span>
          {mcpCountLine(Object.keys(ownServers).length, secretReferences)}
        </span>
        <button
          className="ui-button secondary"
          onClick={() => importInput.current?.click()}
        >
          Import .mcp.json
        </button>
        <input
          ref={importInput}
          type="file"
          accept=".json,application/json"
          aria-label="Import .mcp.json file"
          hidden
          onChange={(event) => {
            void importFile(event.target.files?.[0]);
            event.target.value = "";
          }}
        />
        <button className="ui-button primary" onClick={() => setAdding(true)}>
          <Plus size={14} aria-hidden /> Add server
        </button>
      </div>
      {error ? (
        <p role="alert" className="pd-alert">
          {error}
        </p>
      ) : null}
      {unknown.length ? (
        <p role="alert" className="pd-alert">
          Unknown Environment variable{unknown.length === 1 ? "" : "s"}:{" "}
          {unknown.map((key) => "${" + key + "}").join(", ")}
        </p>
      ) : null}
      {adding ? (
        <div className="pd-editor">
          <input
            className="pd-input"
            aria-label="Server name"
            placeholder="Server name"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
          <textarea
            className="pd-input"
            aria-label="Server definition"
            placeholder={'{"command":"npx","args":[]}'}
            value={definition}
            onChange={(event) => setDefinition(event.target.value)}
          />
          <div className="pd-editor-actions">
            <button
              className="ui-button ghost"
              onClick={() => setAdding(false)}
            >
              Cancel
            </button>
            <button
              className="ui-button primary"
              disabled={busy}
              onClick={() => void addServer()}
            >
              Save server
            </button>
          </div>
        </div>
      ) : null}
      <div className="pd-servers">
        {Object.entries(ownServers).map(([serverName, server]) => {
          const invalid = unknownMcpVariables(server, project.env);
          const disabled = disabledNames.includes(serverName);
          return (
            <article className="pd-server" key={serverName}>
              <div className="pd-server-mark">{markOf(serverName)}</div>
              <div className="pd-server-text">
                <div className="pd-server-head">
                  <strong>{serverName}</strong>
                  <Tag tone="neutral" dot={false}>
                    {server.url ? "http" : "stdio"}
                  </Tag>
                  {(needs[serverName] ?? []).map((text) => (
                    <Tag key={text} tone="warning">
                      {text}
                    </Tag>
                  ))}
                </div>
                <span className="pd-server-line">
                  <Highlighted text={serverLine(server)} />
                </span>
                {invalid.length ? (
                  <span
                    className="pd-server-line pd-server-error plain"
                    role="alert"
                  >
                    Unknown: {invalid.map((key) => "${" + key + "}").join(", ")}
                  </span>
                ) : null}
              </div>
              <Toggle
                checked={!disabled}
                label={`${serverName} server`}
                disabled={busy}
                onChange={() => {
                  const next = new Set(disabledNames);
                  if (disabled) next.delete(serverName);
                  else next.add(serverName);
                  void setDisabledServers([...next].sort());
                }}
              />
            </article>
          );
        })}
        {!Object.keys(ownServers).length ? (
          <p className="pd-empty">No project MCP servers yet.</p>
        ) : null}
      </div>
      <h3 className="pd-group-label">Also in this project</h3>
      <div className="pd-servers">
        {repoServers.map((server) => (
          <article className="pd-server other" key={`repo:${server.name}`}>
            <div className="pd-server-text">
              <div className="pd-server-head">
                <strong>{server.name}</strong>
                <Tag tone="neutral" dot={false}>
                  .mcp.json
                </Tag>
              </div>
              <span className="pd-server-line plain">
                {project.git.url
                  ? "Every host has it through the repo."
                  : "From this folder’s .mcp.json"}
              </span>
            </div>
            <Tag tone="ok">from repo</Tag>
          </article>
        ))}
        {otherServers.map((server) => (
          <article className="pd-server other" key={`server:${server.name}`}>
            <div className="pd-server-text">
              <div className="pd-server-head">
                <strong>{server.name}</strong>
                <Tag tone="neutral" dot={false}>
                  ~/.claude.json
                </Tag>
              </div>
              <span className="pd-server-line plain">
                {[
                  "Your Claude config on this Mac",
                  usesText(usage.servers[claudeToolName(server.name)] ?? 0),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </div>
            <Toggle
              checked={!server.disabled}
              label={`${server.name} server`}
              disabled={busy}
              onChange={() => void toggleServer(server)}
            />
          </article>
        ))}
        {also.plugins.map((plugin) => (
          <article className="pd-server other" key={`plugin:${plugin.name}`}>
            <div className="pd-server-text">
              <div className="pd-server-head">
                <strong>{plugin.name}</strong>
                <Tag tone="neutral" dot={false}>
                  plugin
                </Tag>
              </div>
              <span className="pd-server-line plain">
                {pluginLine({
                  skills: skills.some((skill) => skill.plugin === plugin.name),
                  servers:
                    usage.plugins[claudeToolName(plugin.name)]?.servers
                      .length ?? 0,
                  uses:
                    usageFor(plugin, skills, cwd) +
                    (usage.plugins[claudeToolName(plugin.name)]?.uses ?? 0),
                })}
              </span>
            </div>
            <Toggle
              checked={!plugin.disabled}
              label={`${plugin.name} plugin`}
              disabled={busy}
              onChange={() => void toggleServer({ ...plugin, kind: "Plugin" })}
            />
          </article>
        ))}
        {remote ? (
          <p className="pd-empty">
            Claude plugins and personal settings are shown on this Mac.
          </p>
        ) : null}
      </div>
    </ProjectPage>
  );
}
