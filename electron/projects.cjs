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

function hint(value) {
  return value.length >= 12 ? `••••${value.slice(-4)}` : "••••";
}

class Projects {
  constructor({ userDataDir, safeStorage }) {
    this.projectsFile = path.join(userDataDir, "projects.json");
    this.secretsFile = path.join(userDataDir, "project-secrets.json");
    this.safeStorage = safeStorage;
    this.writeQueue = Promise.resolve();
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
    if (!projects[id]) return null;
    return this.#withHints(projects[id], await this.#read(this.secretsFile));
  }

  #withHints(project, secrets) {
    const safe = this.#public(project);
    safe.env = safe.env.map((entry) => {
      const stored = secrets[`${project.id}:${entry.name}`];
      return { ...entry, hasValue: !!stored, hint: stored?.hint };
    });
    return safe;
  }

  async upsert(input) {
    if (!input || typeof input !== "object" || !String(input.name || "").trim())
      throw new Error("Project name is required.");
    const projects = await this.#read(this.projectsFile);
    if (typeof input.id === "string" && !projects[input.id])
      throw new Error("Unknown project.");
    const id = typeof input.id === "string" ? input.id : randomUUID();
    const env = Array.isArray(input.env) ? input.env : [];
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
      mcp:
        input.mcp && typeof input.mcp === "object" && !Array.isArray(input.mcp)
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
      hosts: projects[id]?.hosts ?? {},
    };
    const previousNames = new Set(
      (projects[id]?.env || []).map((entry) => entry.name),
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

  async delete(id) {
    const projects = await this.#read(this.projectsFile);
    delete projects[id];
    await this.#write(this.projectsFile, projects);
    const secrets = await this.#read(this.secretsFile);
    for (const key of Object.keys(secrets))
      if (key.startsWith(`${id}:`)) delete secrets[key];
    await this.#write(this.secretsFile, secrets);
  }

  async setSecret(id, name, value) {
    const projects = await this.#read(this.projectsFile);
    if (!projects[id]) throw new Error("Unknown project.");
    if (!(projects[id].env || []).some((entry) => entry.name === name))
      throw new Error("Unknown project variable.");
    return this.#setEncryptedValue(id, `${name}`, value);
  }

  async #setEncryptedValue(id, key, value) {
    if (typeof value !== "string") throw new Error("Enter a string value.");
    const projects = await this.#read(this.projectsFile);
    if (!projects[id]) throw new Error("Unknown project.");
    if (!this.safeStorage?.isEncryptionAvailable?.())
      throw new Error("Secure storage is unavailable.");
    if (this.safeStorage.getSelectedStorageBackend?.() === "basic_text")
      throw new Error("Secure storage is unavailable.");
    const secrets = await this.#read(this.secretsFile);
    secrets[`${id}:${key}`] = {
      v: 1,
      ct: this.safeStorage.encryptString(value).toString("base64"),
      hint: hint(value),
    };
    await this.#write(this.secretsFile, secrets);
    return { hasValue: true, hint: hint(value) };
  }

  async setHostSecret(id, name, host, value) {
    if (
      typeof host !== "string" ||
      (host !== "local" && !host.startsWith("ssh:"))
    )
      throw new Error("Invalid project host.");
    const projects = await this.#read(this.projectsFile);
    const project = projects[id];
    if (!project) throw new Error("Unknown project.");
    if (!(project.env || []).some((entry) => entry.name === name))
      throw new Error("Unknown project variable.");
    return this.#setEncryptedValue(id, `${name}@${host}`, value);
  }

  async clearSecret(id, name) {
    const secrets = await this.#read(this.secretsFile);
    for (const key of Object.keys(secrets))
      if (key === `${id}:${name}` || key.startsWith(`${id}:${name}@`))
        delete secrets[key];
    await this.#write(this.secretsFile, secrets);
  }

  async reviewEnvImport(id, entries) {
    const projects = await this.#read(this.projectsFile);
    if (!projects[id]) throw new Error("Unknown project.");
    const secrets = await this.#read(this.secretsFile);
    return entries.map(({ name, value }) => {
      const existing = (projects[id].env || []).find(
        (entry) => entry.name === name,
      );
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

  async agentEnvironments() {
    const projects = await this.#read(this.projectsFile);
    const output = {};
    for (const project of Object.values(projects)) {
      output[project.id] = await this.environmentFor(project.id, "agent");
    }
    return output;
  }

  async mcpEnvironments() {
    const projects = await this.#read(this.projectsFile);
    const output = {};
    for (const project of Object.values(projects)) {
      output[project.id] = await this.environmentFor(project.id, "mcp");
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

  async setHostTrust(id, host, trusted) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    const projects = await this.#read(this.projectsFile);
    const project = projects[id];
    if (!project) throw new Error("Unknown project.");
    project.hosts ||= {};
    project.hosts[host] = {
      ...project.hosts[host],
      trusted: !!trusted,
    };
    await this.#write(this.projectsFile, projects);
    return { trusted: !!trusted };
  }

  async setHostOverrides(id, host, overrides) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    if (!overrides || typeof overrides !== "object" || Array.isArray(overrides))
      throw new Error("Host overrides must be an object.");
    const projects = await this.#read(this.projectsFile);
    const project = projects[id];
    if (!project) throw new Error("Unknown project.");
    project.hosts ||= {};
    project.hosts[host] = {
      ...project.hosts[host],
      overrides,
      trusted: !!project.hosts[host]?.trusted,
    };
    await this.#write(this.projectsFile, projects);
    return project.hosts[host].overrides;
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

module.exports = { Projects, normalizeRemote, atomicWriteJson };
