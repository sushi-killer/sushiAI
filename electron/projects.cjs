const { randomUUID } = require("node:crypto");
const fs = require("node:fs/promises");
const path = require("node:path");
const { remoteUrl } = require("./git-remote.cjs");
const { openProjectDb, transaction } = require("./project-db.cjs");
const { projectSlug, slugOf, uniqueSlug } = require("./project-slug.cjs");

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

/** Where a folder lives, as a project knows it: an SSH host by its endpoint,
 * everything else (This Mac, whichever Herdr socket) as "local". */
function hostOf(endpoint) {
  return typeof endpoint === "string" && endpoint.startsWith("ssh:")
    ? endpoint
    : "local";
}

function hint(value) {
  return value.length >= 12 ? `••••${value.slice(-4)}` : "••••";
}

class Projects {
  constructor({ userDataDir, safeStorage }) {
    this.userDataDir = userDataDir;
    this.db = null;
    this.secretsFile = path.join(userDataDir, "project-secrets.json");
    this.safeStorage = safeStorage;
    this.writeQueue = Promise.resolve();
    this.lock = Promise.resolve();
    // `onSendChange(host)` lets whoever holds values for a host replace them
    // when the owner turns sending to it off (or on again).
    this.onSendChange = null;
    // Anything a project holds was written: the daemons' copies are refreshed.
    this.onChange = null;
  }

  /** Every SSH host the owner added gets a project's values: adding the host
   * is the consent. Only a host the owner switched off for this project
   * (`withheld`) gets none. This machine always does. */
  async sendsValues(id, host) {
    if (host === "local") return true;
    const projects = this.#load();
    return !this.#own(projects, id)?.hosts?.[host]?.withheld;
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

  /** The store, opened on first use (a failed import throws and is tried
   * again on the next call). */
  #open() {
    this.db ||= openProjectDb(this.userDataDir);
    return this.db;
  }

  /** Every project by id, each with `folders` from the attached folder rows. */
  #load() {
    const db = this.#open();
    const projects = Object.create(null);
    for (const row of db.prepare("SELECT id, data FROM projects").all())
      projects[row.id] = { ...JSON.parse(row.data), id: row.id, folders: [] };
    for (const row of db
      .prepare(
        "SELECT host, path, project_id FROM folders WHERE project_id IS NOT NULL ORDER BY rowid",
      )
      .all())
      projects[row.project_id]?.folders.push({
        endpoint: row.host,
        cwd: row.path,
      });
    return projects;
  }

  /** Writes the project map back in one transaction: changed projects are
   * updated, missing ones removed, and folder rows follow each project's
   * `folders` (a folder no project lists any more is only unattached). */
  #save(projects) {
    const db = this.#open();
    transaction(db, () => {
      const stored = new Map(
        db
          .prepare("SELECT id, data FROM projects")
          .all()
          .map((row) => [row.id, row.data]),
      );
      for (const id of stored.keys())
        if (!Object.hasOwn(projects, id))
          db.prepare("DELETE FROM projects WHERE id = ?").run(id);
      const wanted = new Map();
      for (const [id, project] of Object.entries(projects)) {
        const { folders = [], ...rest } = project;
        const data = JSON.stringify(rest);
        if (stored.get(id) !== data)
          db.prepare(
            "INSERT INTO projects(id, data) VALUES(?, ?) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
          ).run(id, data);
        for (const folder of folders)
          wanted.set(JSON.stringify([folder.endpoint, folder.cwd]), id);
      }
      const attached = new Set();
      for (const row of db
        .prepare(
          "SELECT host, path, project_id FROM folders WHERE project_id IS NOT NULL",
        )
        .all()) {
        const key = JSON.stringify([row.host, row.path]);
        attached.add(key);
        if (wanted.get(key) !== row.project_id)
          db.prepare(
            "UPDATE folders SET project_id = ? WHERE host = ? AND path = ?",
          ).run(wanted.get(key) ?? null, row.host, row.path);
      }
      for (const [key, id] of wanted)
        if (!attached.has(key)) {
          const [host, folderPath] = JSON.parse(key);
          db.prepare(
            "INSERT INTO folders(host, path, project_id) VALUES(?, ?, ?) ON CONFLICT(host, path) DO UPDATE SET project_id = excluded.project_id",
          ).run(host, folderPath, id);
        }
    });
    this.#changed();
  }

  /** Remembers what git said about a folder (attached or not), keeping the
   * project it is attached to; an unchanged identity writes nothing. */
  rememberFolder(host, cwd, identity) {
    const db = this.#open();
    const own = db
      .prepare("SELECT * FROM folders WHERE host = ? AND path = ?")
      .get(host, cwd);
    const known = own?.seen_at != null && this.#identity(own, own.subdir || "");
    if (
      known &&
      known.linkedWorktree === !!identity.linkedWorktree &&
      ["remote", "commonDir", "checkout", "subdir", "branch"].every(
        (field) => known[field] === identity[field],
      )
    )
      return;
    db.prepare(
      `INSERT INTO folders(host, path, remote_key, common_dir, checkout, linked_worktree, subdir, branch, seen_at)
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(host, path) DO UPDATE SET remote_key = excluded.remote_key,
           common_dir = excluded.common_dir, checkout = excluded.checkout,
           linked_worktree = excluded.linked_worktree, subdir = excluded.subdir,
           branch = excluded.branch, seen_at = excluded.seen_at`,
    ).run(
      host,
      cwd,
      identity.remote,
      identity.commonDir,
      identity.checkout,
      identity.linkedWorktree ? 1 : 0,
      identity.subdir,
      identity.branch,
      Date.now(),
    );
  }

  /** What was last stored about a folder on a host: its own row, else the row
   * whose checkout contains it (the longest one). Null when nothing fits. */
  storedFolder(host, cwd) {
    const db = this.#open();
    const own = db
      .prepare("SELECT * FROM folders WHERE host = ? AND path = ?")
      .get(host, cwd);
    if (own?.seen_at != null) return this.#identity(own, own.subdir || "");
    let best;
    for (const row of db
      .prepare(
        "SELECT * FROM folders WHERE host = ? AND seen_at IS NOT NULL AND checkout != ''",
      )
      .all(host))
      if (
        (cwd === row.checkout || cwd.startsWith(`${row.checkout}/`)) &&
        row.checkout.length > (best?.checkout.length ?? -1)
      )
        best = row;
    return best
      ? this.#identity(best, cwd.slice(best.checkout.length + 1))
      : null;
  }

  #identity(row, subdir) {
    return {
      remote: row.remote_key || "",
      commonDir: row.common_dir || "",
      checkout: row.checkout || "",
      linkedWorktree: !!row.linked_worktree,
      subdir,
      branch: row.branch || "",
    };
  }

  #write(file, value) {
    const operation = this.writeQueue.then(() => atomicWriteJson(file, value));
    this.writeQueue = operation.catch(() => {});
    // Whatever the project holds changed: every host's copy is refreshed
    // soon, not only at the next task.
    void operation.then(
      () => this.#changed(),
      () => {},
    );
    return operation;
  }

  #changed() {
    clearTimeout(this.changeTimer);
    this.changeTimer = setTimeout(() => this.onChange?.(), 250);
    this.changeTimer.unref?.();
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
    const projects = this.#load();
    const secrets = await this.#read(this.secretsFile);
    return Object.values(projects).map((project) =>
      this.#withHints(project, secrets),
    );
  }

  async get(id) {
    const projects = this.#load();
    const project = this.#own(projects, id);
    if (!project) return null;
    return this.#withHints(project, await this.#read(this.secretsFile));
  }

  #withHints(project, secrets) {
    const safe = this.#public(project);
    safe.slug = slugOf(project);
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
    const projects = this.#load();
    assertRemote(input.git?.url);
    // A project made again from a repository that already has one is that
    // project (the same repository is one project, like attach): creating
    // twice, or from a second window, never makes a duplicate.
    const sameKey = normalizeRemote(input.git?.url);
    if (input.id === undefined && sameKey) {
      const same = Object.values(projects).find(
        (item) => normalizeRemote(item.git?.url) === sameKey,
      );
      if (same) input = { ...input, id: same.id, name: same.name };
    }
    if (input.id !== undefined && !this.#own(projects, input.id))
      throw new Error("Unknown project.");
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
        // Empty means "whatever the remote's own default is": a clone then
        // takes no --branch (a repository whose default is not main works).
        defaultBranch: String(input.git?.defaultBranch ?? ""),
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
        codexAccount: input.sessions?.codexAccount,
        backend: input.sessions?.backend,
      },
      targets: Array.isArray(input.targets) ? input.targets.map(String) : [],
      hosts: existing?.hosts ?? {},
      // Folders attached to the project and what the owner removed from it
      // are kept by their own writers, never by a saved copy.
      folders: existing?.folders ?? [],
      // The folder name on hosts is fixed when the project is made: a rename
      // never moves a checkout.
      slug: existing
        ? slugOf(existing)
        : uniqueSlug(projectSlug(input.name), projects),
      dismissed: existing?.dismissed ?? { env: [], mcp: [] },
    };
    const previousNames = new Set(
      (existing?.env || []).map((entry) => entry.name),
    );
    const nextNames = new Set(project.env.map((entry) => entry.name));
    this.#save({ ...projects, [id]: project });
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
      const projects = this.#load();
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
      // A variable the owner removed is not brought back by the next pull.
      const gone = remove.map(String);
      if (gone.length)
        project.dismissed = {
          mcp: project.dismissed?.mcp || [],
          env: [...new Set([...(project.dismissed?.env || []), ...gone])],
        };
      this.#save(projects);
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
      const projects = this.#load();
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      const servers = { ...(project.mcp?.mcpServers || {}) };
      for (const name of remove) delete servers[String(name)];
      if (remove.length)
        project.dismissed = {
          env: project.dismissed?.env || [],
          mcp: [
            ...new Set([
              ...(project.dismissed?.mcp || []),
              ...remove.map(String),
            ]),
          ],
        };
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
      this.#save(projects);
      for (const entry of added)
        await this.#setEncryptedValue(id, entry.name, entry.value);
      return this.#withHints(project, await this.#read(this.secretsFile));
    });
  }

  /** Adds imported variables and MCP servers to a project, never replacing
   * or removing anything it already has. A variable that exists without a
   * value gets the one the source has. What the owner removed earlier stays
   * out unless `force` asks for it again. Values are stored here; the caller
   * gets names and flags back, no values. */
  mergeImport(
    id,
    { variables = [], servers = {} } = {},
    { force = false } = {},
  ) {
    return this.#locked(async () => {
      const projects = this.#load();
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      const dismissed = {
        env: new Set(force ? [] : project.dismissed?.env || []),
        mcp: new Set(force ? [] : project.dismissed?.mcp || []),
      };
      const have = new Set((project.env || []).map((entry) => entry.name));
      const added = [];
      const filled = [];
      const skipped = [];
      const removed = [];
      const secrets = await this.#read(this.secretsFile);
      for (const entry of variables) {
        const name = String(entry.name);
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) continue;
        if (have.has(name)) {
          // Only a secret is filled: a plain value may be a file's example.
          if (entry.secret && entry.value && !secrets[`${id}:${name}`]) {
            filled.push({ ...entry, name });
          } else skipped.push(name);
          continue;
        }
        if (dismissed.env.has(name)) {
          removed.push(name);
          continue;
        }
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
      const skippedServers = [];
      const removedServers = [];
      for (const [name, definition] of Object.entries(servers))
        if (Object.hasOwn(known, name)) skippedServers.push(name);
        else if (dismissed.mcp.has(name)) removedServers.push(name);
        else {
          known[name] = definition;
          addedServers.push(name);
        }
      if (addedServers.length)
        project.mcp = { ...(project.mcp || {}), mcpServers: known };
      if (force)
        project.dismissed = {
          env: (project.dismissed?.env || []).filter(
            (name) => !added.some((item) => item.name === name),
          ),
          mcp: (project.dismissed?.mcp || []).filter(
            (name) => !addedServers.includes(name),
          ),
        };
      this.#save(projects);
      for (const entry of [...added, ...filled])
        if (entry.value)
          await this.#setEncryptedValue(id, entry.name, entry.value);
      return {
        project: this.#withHints(project, await this.#read(this.secretsFile)),
        addedVariables: added.map(({ name, secret }) => ({
          name,
          secret: !!secret,
        })),
        filledVariables: filled.map(({ name }) => name),
        skippedVariables: skipped,
        removedVariables: removed,
        addedServers,
        skippedServers,
        removedServers,
      };
    });
  }

  async folders(id) {
    const projects = this.#load();
    return [...(this.#own(projects, id)?.folders || [])];
  }

  /** What importing would change, without changing it: names that would be
   * added, filled with a value, or skipped because the owner removed them. */
  async describeImport(id, { variables = [], servers = {} } = {}) {
    const projects = this.#load();
    const project = this.#own(projects, id);
    if (!project) throw new Error("Unknown project.");
    const secrets = await this.#read(this.secretsFile);
    const have = new Set((project.env || []).map((entry) => entry.name));
    const out = {
      newVariables: [],
      fillVariables: [],
      removedVariables: [],
      newServers: [],
      removedServers: [],
    };
    for (const entry of variables) {
      const name = String(entry.name);
      if (have.has(name)) {
        if (entry.secret && entry.value && !secrets[`${id}:${name}`])
          out.fillVariables.push(name);
      } else if ((project.dismissed?.env || []).includes(name))
        out.removedVariables.push(name);
      else out.newVariables.push(name);
    }
    for (const name of Object.keys(servers))
      if (Object.hasOwn(project.mcp?.mcpServers || {}, name)) continue;
      else if ((project.dismissed?.mcp || []).includes(name))
        out.removedServers.push(name);
      else out.newServers.push(name);
    return out;
  }

  /** The one place a folder is mapped to a project, in this order: the
   * project the folder was attached to, the project of its git remote, a
   * project with an attached folder of the same remote on any host, a project
   * with an attached folder of the same repository (common git dir) on that
   * host. A remote or common dir the caller does not know comes from the
   * folder's stored identity. Null when none fits. Read only. */
  async resolveProject(query = {}) {
    const project = this.#find(this.#load(), query);
    return project
      ? this.#withHints(project, await this.#read(this.secretsFile))
      : null;
  }

  /** The id `resolveProject` would answer, without reading secrets. */
  projectIdFor(query = {}) {
    return this.#find(this.#load(), query)?.id || "";
  }

  #find(projects, { host, cwd, remoteKey, commonDir } = {}) {
    const db = this.#open();
    const stored =
      typeof cwd === "string" && cwd && (!remoteKey || !commonDir)
        ? this.storedFolder(host, cwd)
        : null;
    remoteKey ||= stored?.remote || "";
    commonDir ||= stored?.commonDir || "";
    const attached = (sql, ...args) => {
      const row = db.prepare(sql).get(...args);
      return row && Object.hasOwn(projects, row.project_id)
        ? projects[row.project_id]
        : null;
    };
    const byFolder =
      typeof cwd === "string" && cwd
        ? attached(
            "SELECT project_id FROM folders WHERE host = ? AND path = ? AND project_id IS NOT NULL",
            host,
            cwd,
          )
        : null;
    if (byFolder) return byFolder;
    const byRemote = remoteKey
      ? Object.values(projects).find(
          (item) => normalizeRemote(item.git?.url) === remoteKey,
        )
      : null;
    if (byRemote) return byRemote;
    const byAttachedRemote = remoteKey
      ? attached(
          "SELECT project_id FROM folders WHERE remote_key = ? AND project_id IS NOT NULL ORDER BY rowid LIMIT 1",
          remoteKey,
        )
      : null;
    if (byAttachedRemote) return byAttachedRemote;
    return commonDir
      ? attached(
          "SELECT project_id FROM folders WHERE host = ? AND common_dir = ? AND project_id IS NOT NULL ORDER BY rowid LIMIT 1",
          host,
          commonDir,
        )
      : null;
  }

  resolveFolder({ remote, endpoint, cwd } = {}) {
    return this.resolveProject({
      host: hostOf(endpoint),
      cwd,
      remoteKey: normalizeRemote(remote),
    });
  }

  /** Opens the project of a folder, making one when it has none: a folder
   * with a git remote joins the project of that remote, any other folder is
   * known by its host and path. A folder that gains a remote later makes its
   * project follow it. */
  attach({ remote = "", endpoint, cwd, name, commonDir } = {}) {
    if (typeof cwd !== "string" || !cwd)
      throw new Error("Choose a project folder.");
    assertRemote(remote);
    return this.#locked(async () => {
      const projects = this.#load();
      const host = hostOf(endpoint);
      const url = typeof remote === "string" ? remote.trim() : "";
      const known = this.#open()
        .prepare("SELECT common_dir FROM folders WHERE host = ? AND path = ?")
        .get(host, cwd);
      let project = this.#find(projects, {
        host,
        cwd,
        remoteKey: normalizeRemote(url),
        commonDir: commonDir || known?.common_dir || "",
      });
      if (!project) {
        const id = randomUUID();
        project = {
          id,
          name:
            String(name || "")
              .trim()
              .slice(0, 200) ||
            path.basename(cwd) ||
            "Project",
          slug: uniqueSlug(
            projectSlug(
              String(name || "").trim() || path.basename(cwd) || "Project",
            ),
            projects,
          ),
          git: { url, defaultBranch: "" },
          env: [],
          mcp: {},
          setup: { install: "", check: "" },
          network: { allowedDomains: [] },
          sessions: {},
          targets: [host],
          hosts: {},
          folders: [],
          dismissed: { env: [], mcp: [] },
        };
        projects[id] = project;
      } else if (url && !project.git?.url) {
        // The folder had no remote when it was attached; now it has one.
        project.git = { ...project.git, url };
      }
      project.folders ||= [];
      project.slug ||= projectSlug(project.name);
      if (
        !project.folders.some(
          (folder) => folder.endpoint === host && folder.cwd === cwd,
        )
      )
        project.folders.push({ endpoint: host, cwd });
      project.targets = [...new Set([...(project.targets || []), host])];
      this.#save(projects);
      const key = normalizeRemote(url);
      if (key || commonDir)
        this.#open()
          .prepare(
            "UPDATE folders SET remote_key = COALESCE(NULLIF(?, ''), remote_key), common_dir = COALESCE(NULLIF(?, ''), common_dir) WHERE host = ? AND path = ?",
          )
          .run(key, String(commonDir || ""), host, cwd);
      return this.#withHints(project, await this.#read(this.secretsFile));
    });
  }

  delete(id) {
    return this.#locked(async () => {
      const projects = this.#load();
      if (!this.#own(projects, id)) return;
      delete projects[id];
      this.#save(projects);
      const secrets = await this.#read(this.secretsFile);
      for (const key of Object.keys(secrets))
        if (key.startsWith(`${id}:`)) delete secrets[key];
      await this.#write(this.secretsFile, secrets);
    });
  }

  setSecret(id, name, value) {
    return this.#locked(async () => {
      const projects = this.#load();
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
      const projects = this.#load();
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      let entry = (project.env || []).find((item) => item.name === "GIT_TOKEN");
      entry ||= (project.env || []).find(
        (item) => item.name === "GITHUB_TOKEN",
      );
      if (!entry) {
        entry = { name: "GIT_TOKEN", secret: true, availableTo: ["setup"] };
        project.env = [...(project.env || []), entry];
        this.#save(projects);
      }
      await this.#setEncryptedValue(id, entry.name, value);
      return this.#withHints(project, await this.#read(this.secretsFile));
    });
  }

  async #setEncryptedValue(id, key, value) {
    if (typeof value !== "string") throw new Error("Enter a string value.");
    const projects = this.#load();
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
      const projects = this.#load();
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
    const projects = this.#load();
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
    if (!(await this.sendsValues(id, host))) return null;
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
    let remote = "";
    try {
      remote = await remoteUrl(directory, 2000);
    } catch {}
    return this.resolveFolder({ endpoint: "local", cwd: directory, remote });
  }

  /** Whether a project lives on a host: it has a folder there, or the host
   * was one of its targets. A host never gets values of a project it does not
   * run. This Mac runs all of them. */
  #onHost(project, host) {
    return (
      host === "local" ||
      (project.folders || []).some((folder) => folder.endpoint === host) ||
      (project.targets || []).includes(host) ||
      !!project.hosts?.[host]
    );
  }

  async agentEnvironments(host = "local") {
    const projects = this.#load();
    const output = {};
    for (const project of Object.values(projects)) {
      if (!this.#onHost(project, host)) continue;
      output[project.id] = await this.environmentFor(project.id, "agent", host);
    }
    return output;
  }

  async mcpEnvironments(host = "local") {
    const projects = this.#load();
    const output = {};
    for (const project of Object.values(projects)) {
      if (!this.#onHost(project, host)) continue;
      output[project.id] = await this.environmentFor(project.id, "mcp", host);
    }
    return output;
  }

  /** Folder path -> project id on a host, so a task started in a folder
   * without naming its project (the orchestrator agent's own) still gets that
   * project's values. */
  async repoProjects(host = "local") {
    const projects = this.#load();
    const output = {};
    for (const project of Object.values(projects))
      for (const folder of project.folders || [])
        if (folder.endpoint === host) output[folder.cwd] = project.id;
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

  /** "Don't send secrets to this host": the host gets no values of this
   * project, and its daemon drops what it holds. */
  setHostWithheld(id, host, withheld) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    return this.#locked(async () => {
      const projects = this.#load();
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      project.hosts ||= {};
      project.hosts[host] = { ...project.hosts[host], withheld: !!withheld };
      this.#save(projects);
      this.onSendChange?.(host);
      return { withheld: !!withheld };
    });
  }

  setHostOverrides(id, host, overrides) {
    if (typeof host !== "string" || !host.startsWith("ssh:"))
      throw new Error("Invalid project host.");
    if (!overrides || typeof overrides !== "object" || Array.isArray(overrides))
      throw new Error("Host overrides must be an object.");
    return this.#locked(async () => {
      const projects = this.#load();
      const project = this.#own(projects, id);
      if (!project) throw new Error("Unknown project.");
      project.hosts ||= {};
      project.hosts[host] = {
        ...project.hosts[host],
        overrides,
      };
      this.#save(projects);
      return project.hosts[host].overrides;
    });
  }

  async resolve(remote) {
    // A folder is asked for by its host and path too, for one without a remote.
    if (remote && typeof remote === "object" && remote.cwd)
      return this.resolveFolder(remote);
    const key = normalizeRemote(
      typeof remote === "object" ? remote?.remote : remote,
    );
    if (!key) return null;
    return this.resolveProject({ remoteKey: key });
  }
}

module.exports = { Projects, normalizeRemote, assertRemote, atomicWriteJson };
