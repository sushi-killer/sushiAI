const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const { createReadStream } = require("node:fs");
const readline = require("node:readline");
const { randomBytes } = require("node:crypto");

const MAX_NAME = 200;

function projectPath(value) {
  if (
    typeof value !== "string" ||
    !path.isAbsolute(value) ||
    value.length > 4096 ||
    value.includes("\0")
  )
    throw new Error("Choose an existing local project folder.");
  return path.resolve(value);
}

function serverName(value) {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    value.length > MAX_NAME ||
    value.includes("\0")
  )
    throw new Error("Invalid MCP server name.");
  return value;
}

function mapObject(value) {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};
}

function names(value) {
  return Array.isArray(value)
    ? value.filter(
        (name) =>
          typeof name === "string" &&
          name.trim() &&
          name.length <= MAX_NAME &&
          !name.includes("\0"),
      )
    : [];
}

function isClaudeAi(name) {
  return name.toLowerCase().startsWith("claude.ai ");
}

function sourceLabel(source, name) {
  if (isClaudeAi(name)) return "Claude.ai connector";
  if (name === "computer-use") return "Claude Code built-in";
  return (
    {
      local: "This project · local",
      project: "Project · .mcp.json",
      user: "User scope",
      saved: "Saved project choice",
    }[source] || "Claude Code"
  );
}

async function readJson(file, fallback) {
  try {
    const text = await fs.readFile(file, "utf8");
    const value = JSON.parse(text);
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error(`Invalid JSON object in ${file}.`);
    return value;
  } catch (error) {
    if (error?.code === "ENOENT") return fallback;
    if (error instanceof SyntaxError)
      throw new Error(`Could not read Claude Code configuration: ${file}`);
    throw error;
  }
}

function addServer(map, name, source) {
  if (!Object.hasOwn(map, name)) map[name] = source;
}

function projectConfig(config, cwd) {
  const projects = mapObject(config.projects);
  const row = projects[cwd];
  return row && typeof row === "object" && !Array.isArray(row) ? row : {};
}

function listEntries({ config, project, projectFile }) {
  const sources = Object.create(null);
  // Claude Code's precedence is local, project, then user. Keep the first
  // source for a duplicated name so the toggle writes the matching list.
  for (const name of Object.keys(mapObject(project.mcpServers)))
    addServer(sources, name, "local");
  for (const name of Object.keys(mapObject(projectFile.mcpServers)))
    addServer(sources, name, "project");
  for (const name of Object.keys(mapObject(config.mcpServers)))
    addServer(sources, name, "user");

  for (const name of names(project.disabledMcpServers))
    addServer(sources, name, "saved");
  for (const name of names(project.disabledMcpjsonServers))
    addServer(sources, name, "project");

  const disabledServers = new Set(names(project.disabledMcpServers));
  const disabledProjectServers = new Set(names(project.disabledMcpjsonServers));
  return Object.entries(sources)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, source]) => ({
      name,
      source,
      sourceLabel: sourceLabel(source, name),
      disabled:
        source === "project"
          ? disabledProjectServers.has(name)
          : disabledServers.has(name),
    }));
}

async function atomicWriteJson(file, value) {
  let mode = 0o600;
  try {
    const stat = await fs.stat(file);
    mode = stat.mode & 0o777;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporary = `${file}.sushiai-${process.pid}-${Date.now()}-${randomBytes(6).toString("hex")}`;
  try {
    await fs.writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
      encoding: "utf8",
      mode,
      flag: "wx",
    });
    await fs.rename(temporary, file);
  } catch (error) {
    await fs.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
}

class ClaudeMcp {
  constructor({ home = os.homedir() } = {}) {
    this.home = home;
    this.locks = new Map();
  }

  configFile() {
    return path.join(this.home, ".claude.json");
  }

  async load(cwd) {
    const root = projectPath(cwd);
    try {
      const stat = await fs.stat(root);
      if (!stat.isDirectory()) throw new Error("not a directory");
    } catch (error) {
      if (error?.code === "ENOENT" || error?.message === "not a directory")
        throw new Error("Choose an existing local project folder.");
      throw error;
    }
    const [config, projectFile] = await Promise.all([
      readJson(this.configFile(), {}),
      readJson(path.join(root, ".mcp.json"), {}),
    ]);
    const project = projectConfig(config, root);
    return { root, config, project, projectFile };
  }

  /** How often each MCP server was called from this project in the last 30
   * days, counted from Claude Code's own transcripts. A plugin's servers are
   * named `plugin_<plugin>_<server>`; they are counted under the plugin. */
  async usage(cwd, now = Date.now()) {
    const root = projectPath(cwd);
    const directory = path.join(
      this.home,
      ".claude",
      "projects",
      root.replace(/[^A-Za-z0-9]/g, "-"),
    );
    const since = now - 30 * 86400000;
    const servers = {};
    const plugins = {};
    let files = [];
    try {
      files = await fs.readdir(directory);
    } catch {
      return { servers, plugins };
    }
    for (const file of files.filter((name) => name.endsWith(".jsonl"))) {
      const full = path.join(directory, file);
      const stat = await fs.stat(full).catch(() => null);
      if (!stat || stat.mtimeMs < since) continue;
      const lines = readline.createInterface({
        input: createReadStream(full, { encoding: "utf8" }),
        crlfDelay: Infinity,
      });
      for await (const line of lines) {
        if (!line.includes("tool_use") || !line.includes('"mcp__')) continue;
        let at = Number.NaN;
        try {
          at = Date.parse(JSON.parse(line).timestamp);
        } catch {
          continue;
        }
        if (!(at >= since && at <= now)) continue;
        for (const [, tool] of line.matchAll(/"name":"mcp__([^"]+)"/g)) {
          const plugin = /^plugin_(.+?)_(.+?)__/.exec(tool);
          if (plugin) {
            const entry = (plugins[plugin[1]] ||= { uses: 0, servers: [] });
            entry.uses++;
            if (!entry.servers.includes(plugin[2]))
              entry.servers.push(plugin[2]);
          } else {
            const server = tool.split("__")[0];
            servers[server] = (servers[server] || 0) + 1;
          }
        }
      }
    }
    return { servers, plugins };
  }

  async list(cwd) {
    const state = await this.load(cwd);
    return {
      cwd: state.root,
      servers: listEntries(state),
    };
  }

  /** Server definitions a project can adopt: the repo's `.mcp.json` plus this
   * project's own entry in `~/.claude.json` (which wins on a duplicate name).
   * User-scope servers are personal and never imported. */
  async importable(cwd) {
    const state = await this.load(cwd);
    const servers = {};
    for (const source of [state.projectFile, state.project])
      for (const [name, definition] of Object.entries(
        mapObject(source.mcpServers),
      ))
        if (definition && typeof definition === "object")
          servers[name] = definition;
    return { cwd: state.root, servers };
  }

  /** `{mcpServers: {name: definition}}` for `claude --mcp-config`: every
   * enabled server, resolved from whichever source `listEntries` already
   * picked for it. Reuses `load`/`listEntries` rather than re-deriving
   * precedence here. */
  async launchConfig(cwd) {
    const state = await this.load(cwd);
    const servers = {};
    for (const entry of listEntries(state)) {
      if (entry.disabled) continue;
      const definition =
        entry.source === "local"
          ? mapObject(state.project.mcpServers)[entry.name]
          : entry.source === "project"
            ? mapObject(state.projectFile.mcpServers)[entry.name]
            : entry.source === "user"
              ? mapObject(state.config.mcpServers)[entry.name]
              : undefined; // "saved": a disabled marker with no live definition.
      if (definition) servers[entry.name] = definition;
    }
    return { mcpServers: servers };
  }

  async toggle(input) {
    if (
      !input ||
      typeof input !== "object" ||
      Array.isArray(input) ||
      Object.keys(input).some(
        (key) => !["cwd", "name", "source", "disabled"].includes(key),
      )
    )
      throw new Error("Invalid MCP request.");
    const { cwd, name, disabled, source } = input;
    const root = projectPath(cwd);
    const id = serverName(name);
    if (typeof disabled !== "boolean") throw new Error("Invalid MCP state.");
    if (
      source !== undefined &&
      !["local", "project", "user", "saved"].includes(source)
    )
      throw new Error("Invalid MCP source.");
    const lockKey = this.configFile();
    const previous = this.locks.get(lockKey) || Promise.resolve();
    const work = previous
      .catch(() => {})
      .then(async () => {
        const state = await this.load(root);
        const projects = { ...mapObject(state.config.projects) };
        const project = {
          ...projectConfig(state.config, root),
        };
        const visible = listEntries(state).find((server) => server.name === id);
        const projectKey = (source || visible?.source) === "project";
        const key = projectKey
          ? "disabledMcpjsonServers"
          : "disabledMcpServers";
        const otherKey = projectKey
          ? "disabledMcpServers"
          : "disabledMcpjsonServers";
        const current = new Set(names(project[key]));
        const other = new Set(names(project[otherKey]));
        if (disabled) current.add(id);
        else current.delete(id);
        other.delete(id);
        project[key] = [...current].sort((a, b) => a.localeCompare(b));
        project[otherKey] = [...other].sort((a, b) => a.localeCompare(b));
        projects[root] = project;
        await atomicWriteJson(this.configFile(), {
          ...state.config,
          projects,
        });
        return {
          cwd: root,
          servers: listEntries({
            ...state,
            config: { ...state.config, projects },
            project,
          }),
        };
      });
    this.locks.set(lockKey, work);
    try {
      return await work;
    } finally {
      if (this.locks.get(lockKey) === work) this.locks.delete(lockKey);
    }
  }
}

module.exports = {
  ClaudeMcp,
  atomicWriteJson,
  listEntries,
  projectPath,
};
