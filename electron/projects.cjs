const { randomUUID } = require("node:crypto");
const fs = require("node:fs/promises");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execFileAsync = promisify(execFile);

function normalizeRemote(url) {
  if (typeof url !== "string" || !url.trim()) return "";
  return url
    .trim()
    .replace(/^git@([^:]+):/i, "https://$1/")
    .replace(/^ssh:\/\/(?:[^@/]+@)?/i, "https://")
    .replace(/^https?:\/\//i, "")
    .replace(/^[^@/]+@/, "")
    .replace(/^([^/:]+):\d+\//, "$1/")
    .replace(/\.git$/i, "")
    .replace(/\/+$/, "")
    .toLowerCase();
}

/** A git remote is a URL or a path, never an option: `--upload-pack=...` would
 * make git run a command. Every place that hands a user URL to git checks. */
function assertRemote(url) {
  if (typeof url === "string" && url.trim().startsWith("-"))
    throw new Error("A git URL cannot start with a dash.");
}

async function atomicWriteJson(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await fs.writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
      mode: 0o600,
      flag: "wx",
    });
    await fs.rename(temporary, file);
  } catch (error) {
    await fs.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
}

const STAGES = ["setup", "agent", "mcp"];

function hint(value) {
  return value.length >= 12 ? `••••${value.slice(-4)}` : "••••";
}

class Projects {
  constructor({ userDataDir, safeStorage }) {
    this.projectsFile = path.join(userDataDir, "projects.json");
    this.secretsFile = path.join(userDataDir, "project-secrets.json");
    this.safeStorage = safeStorage;
    this.writeQueue = Promise.resolve();
    this.lock = Promise.resolve();
  }

  /** Runs read-modify-write operations on the project list one at a time,
   * so two of them can never both start from the same stale copy. */
  #locked(operation) {
    const run = this.lock.then(operation);
    this.lock = run.catch(() => {});
    return run;
  }

  /** The project stored under `id`, or undefined. Ids come from the
   * renderer: only an own key of the stored map counts, so "__proto__" or
   * "constructor" can never reach Object.prototype or a built-in. */
  #own(projects, id) {
    return typeof id === "string" && Object.hasOwn(projects, id)
      ? projects[id]
      : undefined;
  }

  #write(file, value) {
    const operation = this.writeQueue.then(() => atomicWriteJson(file, value));
    this.writeQueue = operation.catch(() => {});
    return operation;
  }

  async #read(file) {
    try {
      const parsed = JSON.parse(await fs.readFile(file, "utf8"));
      return parsed && typeof parsed === "object" && !Array.isArray(parsed)
        ? parsed
        : {};
    } catch (error) {
      if (error?.code === "ENOENT") return {};
      throw error;
    }
  }

  #public(project) {
    return {
      ...project,
      env: (project.env || []).map(({ name, secret, availableTo, hosts }) => ({
        name,
        secret: !!secret,
        ...(availableTo ? { availableTo } : {}),
        ...(hosts?.length ? { hosts } : {}),
      })),
    };
  }

  async list() {
    const projects = await this.#read(this.projectsFile);
    const secrets = await this.#read(this.secretsFile);
    return Object.values(projects).map((project) =>
      this.#withHints(project, secrets),
    );
  }

  async get(id) {
    const projects = await this.#read(this.projectsFile);
    const project = this.#own(projects, id);
    if (!project) return null;
    return this.#withHints(project, await this.#read(this.secretsFile));
  }

  #withHints(project, secrets) {
    const safe = this.#public(project);
    safe.env = safe.env.map((entry) => {
      const stored = secrets[`${project.id}:${entry.name}`];
      return {
        ...entry,
        hasValue: !!stored,
        hint: stored ? this.#shown(stored, entry.secret) : undefined,
      };
    });
    return safe;
  }

  /** What a stored value shows: a plain variable as it is, a secret only as
   * a mask. Decided now, from the current flag, never from what was stored. */
  #shown(stored, secret) {
    if (secret && /^••••/.test(stored.hint || "")) return stored.hint;
    try {
      const value = this.safeStorage.decryptString(
        Buffer.from(stored.ct, "base64"),
      );
      return secret ? hint(value) : value.slice(0, 200);
    } catch {
      return secret ? "••••" : undefined;
    }
  }

  upsert(input) {
    return this.#locked(() => this.#upsert(input));
  }

  async #upsert(input) {
    if (!input || typeof input !== "object" || !String(input.name || "").trim())
      throw new Error("Project name is required.");
    const projects = await this.#read(this.projectsFile);
    if (input.id !== undefined && !this.#own(projects, input.id))
      throw new Error("Unknown project.");
    assertRemote(input.git?.url);
    const id = typeof input.id === "string" ? input.id : randomUUID();
    // Variables and MCP servers of an existing project change only through
    // updateEnv and updateMcp, which read the stored state under the lock: a
    // stale copy of the project must not be able to overwrite an import.
    const existing = this.#own(projects, id);
    const env = existing
      ? existing.env || []
      : Array.isArray(input.env)
        ? input.env
        : [];
    const project = {
      id,
      name: String(input.name).trim().slice(0, 200),
      git: {
        url: String(input.git?.url || ""),
        defaultBranch: String(input.git?.defaultBranch || "main"),
      },
      env: env
        .map((entry) => ({
          name: String(entry.name || ""),
          secret: !!entry.secret,
          availableTo: Array.isArray(entry.availableTo)
            ? entry.availableTo
            : undefined,
          hosts: Array.isArray(entry.hosts)
            ? entry.hosts.map(String)
            : undefined,
        }))
        .filter((entry) => entry.name),
      mcp: existing
        ? existing.mcp || {}
        : input.mcp &&
            typeof input.mcp === "object" &&
            !Array.isArray(input.mcp)
          ? input.mcp
          : {},
      setup: {
        install: String(input.setup?.install || ""),
        check: String(input.setup?.check || ""),
      },
      network: {
        allowedDomains: Array.isArray(input.network?.allowedDomains)
          ? input.network.allowedDomains.map(String)
          : [],
      },
      sessions: {
        claudeAccount: input.sessions?.claudeAccount,
        backend: input.sessions?.backend,
      },
      targets: Array.isArray(input.targets) ? input.targets.map(String) : [],
      hosts: existing?.hosts ?? {},
    };
    const previousNames = new Set(
      (existing?.env || []).map((entry) => entry.name),
    );
    const nextNames = new Set(project.env.map((entry) => entry.name));
    await this.#write(this.projectsFile, { ...projects, [id]: project });
    if ([...previousNames].some((name) => !nextNames.has(name))) {
      const secrets = await this.#read(this.secretsFile);
      for (const name of previousNames)
        if (!nextNames.has(name))
          for (const key of Object.keys(secrets))
            if (key === `${id}:${name}` || key.startsWith(`${id}:${name}@`))
              delete secrets[key];
      await this.#write(this.secretsFile, secrets);
    }
    return this.#withHints(project, await this.#read(this.secretsFile));
  }

  /** Adds, changes or removes variables on fresh stored state. A secret made
   * plain loses its stored value: it was never meant to be shown, so it is
   * never handed back; the user enters it again. */
  updateEnv(id, { set = [], remove = [] } = {}) {
    return this.#locked(async () => {
      if (!Array.isArray(set) || !Array.isArray(remove))
        throw new Error("Variable changes must be lists.");
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      const env = [...(project.env || [])];
      const dropped = new Set(remove.map(String));
      for (const item of set) {
        const name = String(item?.name || "");
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
          throw new Error(
            "A variable name is letters, digits and underscores.",
          );
        if (
          Array.isArray(item.availableTo) &&
          !item.availableTo.every((stage) => STAGES.includes(stage))
        )
          throw new Error("A variable is available to setup, agent or mcp.");
        const at = env.findIndex((entry) => entry.name === name);
        const next = {
          name,
          secret: !!item.secret,
          availableTo: Array.isArray(item.availableTo)
            ? item.availableTo.map(String)
            : at >= 0
              ? env[at].availableTo
              : ["setup", "agent"],
          hosts: Array.isArray(item.hosts)
            ? item.hosts.map(String)
            : at >= 0
              ? env[at].hosts
              : undefined,
        };
        if (at < 0) env.push(next);
        else {
          if (env[at].secret && !next.secret) dropped.add(name);
          env[at] = next;
        }
      }
      project.env = env.filter(
        (entry) =>
          !dropped.has(entry.name) ||
          set.some((item) => item?.name === entry.name),
      );
      await this.#write(this.projectsFile, projects);
      const secrets = await this.#read(this.secretsFile);
      for (const name of dropped)
        for (const key of Object.keys(secrets))
          if (key === `${id}:${name}` || key.startsWith(`${id}:${name}@`))
            delete secrets[key];
      await this.#write(this.secretsFile, secrets);
      return this.#withHints(project, secrets);
    });
  }

  /** Adds, replaces or removes MCP servers (and sets which are disabled) on
   * fresh stored state. `variables` are secrets the servers now refer to. */
  updateMcp(id, { set = {}, remove = [], disabled, variables = [] } = {}) {
    return this.#locked(async () => {
      if (
        !set ||
        typeof set !== "object" ||
        Array.isArray(set) ||
        !Array.isArray(remove) ||
        !Array.isArray(variables) ||
        [...Object.keys(set), ...remove].some((name) => name === "__proto__")
      )
        throw new Error("Invalid MCP server change.");
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      const servers = { ...(project.mcp?.mcpServers || {}) };
      for (const name of remove) delete servers[String(name)];
      Object.assign(servers, set);
      project.mcp = {
        ...(project.mcp || {}),
        mcpServers: servers,
        ...(Array.isArray(disabled)
          ? { disabledMcpServers: disabled.map(String) }
          : {}),
      };
      const have = new Set((project.env || []).map((entry) => entry.name));
      const added = variables.filter((entry) => !have.has(entry.name));
      project.env = [
        ...(project.env || []),
        ...added.map((entry) => ({
          name: entry.name,
          secret: true,
          availableTo: entry.availableTo || ["mcp"],
        })),
      ];
      await this.#write(this.projectsFile, projects);
      for (const entry of added)
        await this.#setEncryptedValue(id, entry.name, entry.value);
      return this.#withHints(project, await this.#read(this.secretsFile));
    });
  }

  /** Adds imported variables and MCP servers to a project, never replacing
   * or removing anything it already has: names that exist are left alone.
   * Values are stored here; the caller gets names and flags back, no values. */
  mergeImport(id, { variables = [], servers = {} } = {}) {
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      const have = new Set((project.env || []).map((entry) => entry.name));
      const added = [];
      for (const entry of variables) {
        const name = String(entry.name);
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || have.has(name)) continue;
        have.add(name);
        project.env = [
          ...(project.env || []),
          {
            name,
            secret: !!entry.secret,
            availableTo: Array.isArray(entry.availableTo)
              ? entry.availableTo
              : ["setup", "agent"],
          },
        ];
        added.push({ ...entry, name });
      }
      const known = { ...(project.mcp?.mcpServers || {}) };
      const addedServers = [];
      for (const [name, definition] of Object.entries(servers))
        if (!Object.hasOwn(known, name)) {
          known[name] = definition;
          addedServers.push(name);
        }
      if (addedServers.length)
        project.mcp = { ...(project.mcp || {}), mcpServers: known };
      await this.#write(this.projectsFile, projects);
      for (const entry of added)
        if (entry.value)
          await this.#setEncryptedValue(id, entry.name, entry.value);
      return {
        project: this.#withHints(project, await this.#read(this.secretsFile)),
        addedVariables: added.map(({ name, secret }) => ({
          name,
          secret: !!secret,
        })),
        addedServers,
      };
    });
  }

  delete(id) {
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      if (!this.#own(projects, id)) return;
      delete projects[id];
      await this.#write(this.projectsFile, projects);
      const secrets = await this.#read(this.secretsFile);
      for (const key of Object.keys(secrets))
        if (key.startsWith(`${id}:`)) delete secrets[key];
      await this.#write(this.secretsFile, secrets);
    });
  }

  setSecret(id, name, value) {
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      if (!(project.env || []).some((entry) => entry.name === name))
        throw new Error("Unknown project variable.");
      return this.#setEncryptedValue(id, `${name}`, value);
    });
  }

  /** Stores the token a clone uses. A project that keeps none yet gets a
   * GIT_TOKEN variable (setup only) first, so there is always somewhere for
   * it to go. */
  setGitToken(id, value) {
    return this.#locked(async () => {
      if (typeof value !== "string" || !value.trim())
        throw new Error("Enter a token.");
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      let entry = (project.env || []).find((item) => item.name === "GIT_TOKEN");
      entry ||= (project.env || []).find(
        (item) => item.name === "GITHUB_TOKEN",
      );
      if (!entry) {
        entry = { name: "GIT_TOKEN", secret: true, availableTo: ["setup"] };
        project.env = [...(project.env || []), entry];
        await this.#write(this.projectsFile, projects);
      }
      await this.#setEncryptedValue(id, entry.name, value);
      return this.#withHints(project, await this.#read(this.secretsFile));
    });
  }

  async #setEncryptedValue(id, key, value) {
    if (typeof value !== "string") throw new Error("Enter a string value.");
    const projects = await this.#read(this.projectsFile);
    const owner = this.#own(projects, id);
    if (!owner) throw new Error("Unknown project.");
    if (!this.safeStorage?.isEncryptionAvailable?.())
      throw new Error("Secure storage is unavailable.");
    if (this.safeStorage.getSelectedStorageBackend?.() === "basic_text")
      throw new Error("Secure storage is unavailable.");
    const secrets = await this.#read(this.secretsFile);
    const entry = (owner.env || []).find(
      (item) => item.name === key.split("@")[0],
    );
    secrets[`${id}:${key}`] = {
      v: 1,
      ct: this.safeStorage.encryptString(value).toString("base64"),
      // Only ever the masked form on disk: whether a value may be shown is
      // decided from the variable's current secret flag when it is read.
      hint: hint(value),
    };
    await this.#write(this.secretsFile, secrets);
    return {
      hasValue: true,
      hint: entry && !entry.secret ? value.slice(0, 200) : hint(value),
    };
  }

  setHostSecret(id, name, host, value) {
    if (
      typeof host !== "string" ||
      (host !== "local" && !host.startsWith("ssh:"))
    )
      throw new Error("Invalid project host.");
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      if (!(project.env || []).some((entry) => entry.name === name))
        throw new Error("Unknown project variable.");
      return this.#setEncryptedValue(id, `${name}@${host}`, value);
    });
  }

  clearSecret(id, name) {
    return this.#locked(async () => {
      const secrets = await this.#read(this.secretsFile);
      for (const key of Object.keys(secrets))
        if (key === `${id}:${name}` || key.startsWith(`${id}:${name}@`))
          delete secrets[key];
      await this.#write(this.secretsFile, secrets);
    });
  }

  async reviewEnvImport(id, entries) {
    const projects = await this.#read(this.projectsFile);
    const project = this.#own(projects, id);
    if (!project) throw new Error("Unknown project.");
    const secrets = await this.#read(this.secretsFile);
    return entries.map(({ name, value }) => {
      const existing = (project.env || []).find((entry) => entry.name === name);
      if (!existing) return { name, status: "new" };
      const stored = secrets[`${id}:${name}`];
      let previous;
      try {
        previous =
          stored &&
          this.safeStorage.decryptString(Buffer.from(stored.ct, "base64"));
      } catch {}
      return {
        name,
        status:
          previous === undefined
            ? "exists"
            : previous === value
              ? "same"
              : "differs",
      };
    });
  }

  async secretFor(id, name) {
    const secrets = await this.#read(this.secretsFile);
    const stored = secrets[`${id}:${name}`];
    if (!stored) return null;
    try {
      return this.safeStorage.decryptString(Buffer.from(stored.ct, "base64"));
    } catch {
      return null;
    }
  }

  async secretForHost(id, name, host) {
    const project = await this.get(id);
    if (host !== "local" && !project?.hosts?.[host]?.trusted) return null;
    const override = await this.#secretValue(id, `${name}@${host}`);
    return override ?? this.secretFor(id, name);
  }

  async environmentFor(id, stage = "agent", host = "local") {
    const project = await this.get(id);
    if (!project) return {};
    const env = {};
    for (const entry of project.env) {
      const availableTo = entry.availableTo || ["setup", "agent"];
      if (!availableTo.includes(stage)) continue;
      const value = await this.secretForHost(id, entry.name, host);
      if (value !== null) env[entry.name] = value;
    }
    return env;
  }

  async resolveDirectory(directory) {
    try {
      const { stdout } = await execFileAsync(
        "git",
        ["remote", "get-url", "origin"],
        {
          cwd: directory,
          timeout: 2000,
          maxBuffer: 10000,
        },
      );
      return this.resolve(stdout.trim());
    } catch {
      return null;
    }
  }

  async agentEnvironments(host = "local") {
    const projects = await this.#read(this.projectsFile);
    const output = {};
    for (const project of Object.values(projects)) {
      output[project.id] = await this.environmentFor(project.id, "agent", host);
    }
    return output;
  }

  async mcpEnvironments(host = "local") {
    const projects = await this.#read(this.projectsFile);
    const output = {};
    for (const project of Object.values(projects)) {
      output[project.id] = await this.environmentFor(project.id, "mcp", host);
    }
    return output;
  }

  async #secretValue(id, key) {
    const secrets = await this.#read(this.secretsFile);
    const stored = secrets[`${id}:${key}`];
    if (!stored) return null;
    try {
      return this.safeStorage.decryptString(Buffer.from(stored.ct, "base64"));
    } catch {
      return null;
    }
  }

  setHostTrust(id, host, trusted) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      project.hosts ||= {};
      project.hosts[host] = {
        ...project.hosts[host],
        trusted: !!trusted,
      };
      await this.#write(this.projectsFile, projects);
      return { trusted: !!trusted };
    });
  }

  setHostOverrides(id, host, overrides) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    if (!overrides || typeof overrides !== "object" || Array.isArray(overrides))
      throw new Error("Host overrides must be an object.");
    return this.#locked(async () => {
      const projects = await this.#read(this.projectsFile);
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      project.hosts ||= {};
      project.hosts[host] = {
        ...project.hosts[host],
        overrides,
        trusted: !!project.hosts[host]?.trusted,
      };
      await this.#write(this.projectsFile, projects);
      return project.hosts[host].overrides;
    });
  }

  async resolve(remote) {
    const key = normalizeRemote(
      typeof remote === "object" ? remote?.remote : remote,
    );
    if (!key) return null;
    const projects = await this.#read(this.projectsFile);
    const found = Object.values(projects).find(
      (project) => normalizeRemote(project.git?.url) === key,
    );
    return found
      ? this.#withHints(found, await this.#read(this.secretsFile))
      : null;
  }
}

module.exports = { Projects, normalizeRemote, assertRemote, atomicWriteJson };
