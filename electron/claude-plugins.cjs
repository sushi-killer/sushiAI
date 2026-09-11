const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs/promises");
const { randomBytes } = require("node:crypto");

const MAX_NAME = 300;

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

function mapObject(value) {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};
}

function pluginName(value) {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    value.length > MAX_NAME ||
    value.includes("\0")
  )
    throw new Error("Invalid Claude Code plugin name.");
  return value;
}

function sourceLabel(source) {
  return (
    {
      local: "Project · local override",
      project: "Project · shared",
      user: "User scope",
      installed: "Installed plugin",
    }[source] || "Claude Code"
  );
}

async function readJson(file, fallback) {
  try {
    const value = JSON.parse(await fs.readFile(file, "utf8"));
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

async function atomicWriteJson(file, value) {
  let mode = 0o600;
  try {
    mode = (await fs.stat(file)).mode & 0o777;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.sushiai-${process.pid}-${Date.now()}-${randomBytes(6).toString("hex")}`;
  try {
    await fs.writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
      encoding: "utf8",
      mode,
      flag: "wx",
    });
    await fs.chmod(temporary, mode);
    await fs.rename(temporary, file);
  } catch (error) {
    await fs.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
}

function pluginEntries({ user, project, local, installed }) {
  const sources = Object.create(null);
  const add = (name, source) => {
    if (
      typeof name === "string" &&
      name.trim() &&
      name.length <= MAX_NAME &&
      !name.includes("\0") &&
      !Object.hasOwn(sources, name)
    )
      sources[name] = source;
  };
  for (const name of Object.keys(mapObject(local.enabledPlugins)))
    add(name, "local");
  for (const name of Object.keys(mapObject(project.enabledPlugins)))
    add(name, "project");
  for (const name of Object.keys(mapObject(user.enabledPlugins)))
    add(name, "user");
  for (const name of Object.keys(mapObject(installed.plugins)))
    add(name, "installed");

  const enabled = (settings) => mapObject(settings.enabledPlugins);
  return Object.entries(sources)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, source]) => {
      const value =
        source === "local"
          ? enabled(local)[name]
          : source === "project"
            ? enabled(project)[name]
            : source === "user"
              ? enabled(user)[name]
              : (enabled(local)[name] ??
                enabled(project)[name] ??
                enabled(user)[name]);
      return {
        name,
        source,
        sourceLabel: sourceLabel(source),
        disabled: value !== true,
      };
    });
}

class ClaudePlugins {
  constructor({ home = os.homedir() } = {}) {
    this.home = home;
    this.locks = new Map();
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
    const [user, project, local, installed] = await Promise.all([
      readJson(path.join(this.home, ".claude/settings.json"), {}),
      readJson(path.join(root, ".claude/settings.json"), {}),
      readJson(path.join(root, ".claude/settings.local.json"), {}),
      readJson(
        path.join(this.home, ".claude/plugins/installed_plugins.json"),
        {},
      ),
    ]);
    return { root, user, project, local, installed };
  }

  async list(cwd) {
    const state = await this.load(cwd);
    return {
      cwd: state.root,
      plugins: pluginEntries(state),
    };
  }

  async toggle(input) {
    if (
      !input ||
      typeof input !== "object" ||
      Array.isArray(input) ||
      Object.keys(input).some(
        (key) => !["cwd", "name", "disabled"].includes(key),
      )
    )
      throw new Error("Invalid plugin request.");
    const { cwd, name, disabled } = input;
    const root = projectPath(cwd);
    const id = pluginName(name);
    if (typeof disabled !== "boolean") throw new Error("Invalid plugin state.");
    const previous = this.locks.get(root) || Promise.resolve();
    const work = previous
      .catch(() => {})
      .then(async () => {
        const state = await this.load(root);
        const settings = { ...state.local };
        settings.enabledPlugins = {
          ...mapObject(settings.enabledPlugins),
          [id]: !disabled,
        };
        await atomicWriteJson(
          path.join(root, ".claude/settings.local.json"),
          settings,
        );
        return {
          cwd: state.root,
          plugins: pluginEntries({ ...state, local: settings }),
        };
      });
    this.locks.set(root, work);
    try {
      return await work;
    } finally {
      if (this.locks.get(root) === work) this.locks.delete(root);
    }
  }
}

module.exports = { ClaudePlugins, atomicWriteJson, pluginEntries, projectPath };
