const fs = require("node:fs/promises");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execFileAsync = promisify(execFile);
const { createWorktree } = require("../worktree.cjs");
const { localCreate, pullMessage } = require("../project-git.cjs");
const { randomUUID } = require("node:crypto");
const {
  isSecretName,
  mergeEnvSources,
  parseEnv,
  isSecret,
  secretizeMcpServers,
} = require("../project-import.cjs");
const { normalizeRemote, assertRemote } = require("../projects.cjs");
const {
  hostProbeScript,
  readiness,
  prepareScript,
  prepareSteps,
  installScript,
  projectSlug,
  LOCK_FILES,
  installFor,
} = require("../project-hosts.cjs");

function registerProjectIpc({
  handle,
  getConnections,
  getPreview,
  getClaudeMcp,
  terminals,
  terminalPending,
  projects,
}) {
  const connections = () => {
    const value = getConnections();
    if (!value) throw new Error("Connections are not ready.");
    return value;
  };

  if (projects) {
    handle("projects:source-inspect", async (url) => {
      if (typeof url !== "string" || !url.trim())
        throw new Error("Enter a git URL.");
      const remote = url.trim();
      assertRemote(remote);
      const { stdout } = await execFileAsync(
        "git",
        ["ls-remote", "--symref", "--", remote, "HEAD"],
        { timeout: 30000, maxBuffer: 1024 * 1024 },
      );
      const branch =
        stdout.match(/^ref: refs\/heads\/(.+)\tHEAD$/m)?.[1] || "main";
      const directory = await fs.mkdtemp(
        path.join(require("node:os").tmpdir(), "sushiai-project-source-"),
      );
      try {
        await execFileAsync(
          "git",
          [
            "clone",
            "--depth",
            "1",
            "--branch",
            branch,
            "--",
            remote,
            directory,
          ],
          {
            timeout: 2 * 60 * 1000,
            maxBuffer: 1024 * 1024,
          },
        );
        const read = async (file) => {
          try {
            return await fs.readFile(path.join(directory, file), "utf8");
          } catch (error) {
            if (error?.code === "ENOENT") return "";
            throw error;
          }
        };
        const envExample = await read(".env.example");
        const mcp = await read(".mcp.json");
        let lockFile = "";
        for (const [file] of LOCK_FILES) {
          try {
            await fs.access(path.join(directory, file));
            lockFile = file;
            break;
          } catch {}
        }
        return {
          branch,
          envExample,
          mcp,
          lockFile,
          install: installFor(lockFile),
        };
      } finally {
        await fs.rm(directory, { recursive: true, force: true });
      }
    });
    handle("projects:local-create", (input) => localCreate(input));
    handle("projects:local-install", async (id, cwd) => {
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      if (!project.setup?.install) return { ran: false, seconds: 0 };
      if (typeof cwd !== "string" || !cwd.startsWith("/"))
        throw new Error("Choose an absolute project folder.");
      // Only a git checkout is a project folder to install into; anything
      // else is left alone.
      const inside = await execFileAsync(
        "git",
        ["rev-parse", "--is-inside-work-tree"],
        { cwd, timeout: 5000 },
      )
        .then(({ stdout }) => stdout.trim() === "true")
        .catch(() => false);
      if (!inside) return { ran: false, seconds: 0 };
      const began = Date.now();
      await execFileAsync(
        "/bin/sh",
        ["-c", `set -e\n${installScript(project)}`],
        {
          cwd,
          env: {
            ...process.env,
            ...(await projects.environmentFor(id, "setup")),
          },
          timeout: 15 * 60 * 1000,
          maxBuffer: 4 * 1024 * 1024,
        },
      ).catch((error) => {
        const detail = String(error.stderr || error.message || "")
          .trim()
          .split("\n")
          .slice(-3)
          .join(" ");
        throw new Error(`${project.setup.install} failed: ${detail}`);
      });
      return { ran: true, seconds: Math.round((Date.now() - began) / 1000) };
    });
    handle("projects:list", () => projects.list());
    handle("projects:get", (id) => projects.get(id));
    // What a source holds for a project that does not exist yet, by token.
    const pendingImports = new Map();
    handle("projects:upsert", async (input) => {
      const { importToken, ...project } = input || {};
      // What a source held goes only into the project this very call creates,
      // and only into entries that are secrets; the token is used up either way.
      const creating = !project.id;
      const saved = await projects.upsert(project);
      const held = importToken ? pendingImports.get(importToken) : undefined;
      if (importToken) pendingImports.delete(importToken);
      if (held && creating) {
        for (const [name, value] of Object.entries(held.values))
          if (saved.env.some((entry) => entry.name === name && entry.secret))
            await projects.setSecret(saved.id, name, value);
        return projects.get(saved.id);
      }
      return saved;
    });
    handle("projects:git-token:set", (id, value) =>
      projects.setGitToken(id, value),
    );
    handle("projects:env:update", (id, change) =>
      projects.updateEnv(id, change),
    );
    // Servers written by hand get the same treatment as imported ones: a
    // literal credential becomes a ${VAR} reference with a secret variable.
    handle("projects:mcp:update", async (id, change) => {
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      const set =
        change?.set &&
        typeof change.set === "object" &&
        !Array.isArray(change.set)
          ? change.set
          : {};
      const cleaned = secretizeMcpServers(set, await takenValues(project));
      return projects.updateMcp(id, {
        set: cleaned.servers,
        remove: Array.isArray(change?.remove) ? change.remove : [],
        disabled: change?.disabled,
        variables: Object.entries(cleaned.secrets).map(([name, value]) => ({
          name,
          value,
        })),
      });
    });
    handle("projects:delete", (id) => projects.delete(id));
    handle("projects:secret:set", (id, name, value) =>
      projects.setSecret(id, name, value),
    );
    handle("projects:host:secret:set", (id, name, host, value) =>
      projects.setHostSecret(id, name, host, value),
    );
    handle("projects:secret:clear", (id, name) =>
      projects.clearSecret(id, name),
    );
    handle("projects:host:trust", (id, host, trusted) =>
      projects.setHostTrust(id, host, trusted),
    );
    handle("projects:host:overrides", (id, host, overrides) =>
      projects.setHostOverrides(id, host, overrides),
    );
    handle("projects:host:check", async (id, host, cwd) => {
      if (typeof host !== "string" || !host.startsWith("ssh:"))
        throw new Error("Invalid project host.");
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      const output = await connections().exec(
        host,
        hostProbeScript(cwd, project.name, project),
        {
          timeout: 40000,
        },
      );
      return readiness({ output, project: { ...project, host }, cwd });
    });
    handle("projects:host:prepare", async (id, host, useHostLogin = false) => {
      if (typeof host !== "string" || !host.startsWith("ssh:"))
        throw new Error("Invalid project host.");
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      if (!project.hosts?.[host]?.trusted)
        throw new Error("Trust this host before sending project values.");
      // The clone token is GIT_TOKEN, or GITHUB_TOKEN when that is what the
      // project already keeps.
      const tokenEntry =
        project.env.find((entry) => entry.name === "GIT_TOKEN") ||
        project.env.find((entry) => entry.name === "GITHUB_TOKEN");
      const token =
        useHostLogin || !tokenEntry
          ? ""
          : await projects.secretForHost(id, tokenEntry.name, host);
      // A folder at the standard path that belongs to another repository is
      // never reused: nothing is installed there and no secret goes near it.
      const slug = projectSlug(project.name);
      const refuse = (message) => ({
        ok: false,
        stage: "clone",
        message,
        steps: prepareSteps(project, "", "clone"),
      });
      // Only an absent folder or a checkout of this very repository is used.
      const state = await connections()
        .exec(
          host,
          `t="$HOME/sushiai/${slug}"; if [ ! -e "$t" ]; then echo ABSENT; elif [ -e "$t/.git" ]; then o=$(git -C "$t" remote get-url origin 2>/dev/null) && echo "ORIGIN=$o" || echo NOORIGIN; else echo NOTREPO; fi`,
          { timeout: 30000 },
        )
        .then((out) => out.trim().split("\n").pop() || "")
        .catch(() => "");
      if (state === "NOTREPO")
        return refuse(
          `~/sushiai/${slug} on this host is not a git checkout, so nothing was installed there.`,
        );
      if (state === "NOORIGIN")
        return refuse(
          `~/sushiai/${slug} on this host is a git folder without an origin, so nothing was installed there.`,
        );
      if (state.startsWith("ORIGIN=")) {
        const present = state.slice("ORIGIN=".length);
        if (normalizeRemote(present) !== normalizeRemote(project.git?.url))
          return refuse(
            `~/sushiai/${slug} on this host is a checkout of a different repository (${present}).`,
          );
      } else if (state !== "ABSENT")
        return refuse(
          `Could not look at ~/sushiai/${slug} on this host, so nothing was sent.`,
        );
      // The clone token goes in on its own; the rest of the setup variables
      // reach the install and check steps over stdin.
      const setupEnv = await projects.environmentFor(id, "setup", host);
      if (tokenEntry) delete setupEnv[tokenEntry.name];
      const prepared = prepareScript(project, token, setupEnv);
      try {
        const output = await connections().exec(host, prepared.script, {
          input: prepared.input,
          timeout: 15 * 60 * 1000,
        });
        const path =
          output.match(/^SUSHIAI_PREPARED=(.+)$/m)?.[1] || prepared.path;
        const pull =
          output.match(/^SUSHIAI_PULL=(.+)$/m)?.[1]?.trim() || "current";
        return {
          ok: true,
          path,
          output,
          pull,
          message: pullMessage(pull),
          steps: prepareSteps(project, output),
        };
      } catch (error) {
        const timedOut = !!error?.timedOut;
        // A timeout carries what the host had printed by then.
        const text = timedOut
          ? String(error.stderr || "")
          : error instanceof Error
            ? error.message
            : String(error);
        // The script says last which step it was in; noise cannot push that
        // out of the tail the transport keeps. Failing that, the last step it
        // started. With neither, the step after the last one that finished:
        // a clone that finished is never blamed.
        const failedAt = [...text.matchAll(/^SUSHIAI_FAILED=(\w+)$/gm)];
        const stages = [...text.matchAll(/^SUSHIAI_STAGE=(\w+)$/gm)];
        const finished = [...text.matchAll(/^SUSHIAI_STEP=(\w+):/gm)].map(
          (match) => match[1],
        );
        const planned = prepareSteps(project, "", "clone").map(
          (step) => step.id,
        );
        const next = planned.find((id) => !finished.includes(id));
        const failed =
          failedAt.at(-1)?.[1] ??
          stages.at(-1)?.[1] ??
          (finished.includes("clone") ? (next ?? "check") : "clone");
        const stage = failed === "clone" ? "clone" : "setup";
        // Only a clone's 401 or 403 is about the git token; npm's is not, and
        // neither is a timeout.
        const status =
          stage === "clone" && !timedOut
            ? text.match(/\b(401|403)\b/)?.[1]
            : undefined;
        return {
          ok: false,
          stage,
          timedOut: timedOut || undefined,
          status: status ? Number(status) : undefined,
          message: timedOut
            ? `Timed out after 15 minutes during ${failed}.`
            : text,
          steps: prepareSteps(project, text, failed),
        };
      }
    });
    handle("projects:env:review-text", async (id, text) => {
      if (typeof text !== "string") throw new Error("Choose a .env file.");
      const entries = parseEnv(text);
      const statuses = await projects.reviewEnvImport(id, entries);
      return entries.map((entry) => ({
        ...entry,
        secret: isSecret(entry.name, entry.value),
        status: statuses.find((item) => item.name === entry.name)?.status,
      }));
    });
    handle("projects:env:classify", (names) =>
      (Array.isArray(names) ? names : []).map((name) =>
        isSecretName(String(name)),
      ),
    );

    // Reading a project's own files into the project happens here, so the
    // values never reach the renderer: it gets back names and flags.
    const readFile = async (endpoint, root, file) => {
      if (!endpoint || !root) return "";
      const found = await connections()
        .inspect(endpoint, { operation: "read", root, path: file })
        .catch(() => null);
      return found?.base64
        ? Buffer.from(found.base64, "base64").toString("utf8")
        : "";
    };
    // Variable names the project already has, with their stored values, so
    // importing the same literal again reuses the name instead of adding one.
    const takenValues = async (project) =>
      Object.fromEntries(
        await Promise.all(
          project.env.map(async (entry) => [
            entry.name,
            (await projects.secretFor(project.id, entry.name)) ?? "\0",
          ]),
        ),
      );
    const collect = async ({
      endpoint,
      root,
      local,
      example,
      repoMcp,
      taken,
    }) => {
      const [exampleText, envText, localText] = await Promise.all([
        example === undefined
          ? readFile(endpoint, root, ".env.example")
          : example,
        readFile(endpoint, root, ".env"),
        readFile(endpoint, root, ".env.local"),
      ]);
      const variables = mergeEnvSources({
        example: exampleText,
        env: envText,
        local: localText,
      });
      const merged = { ...repoMcp };
      if (endpoint && root) {
        if (local)
          Object.assign(
            merged,
            (
              await getClaudeMcp()
                .importable(root)
                .catch(() => null)
            )?.servers,
          );
        else
          try {
            Object.assign(
              merged,
              JSON.parse(await readFile(endpoint, root, ".mcp.json"))
                .mcpServers,
            );
          } catch {}
      }
      const { servers, secrets } = secretizeMcpServers(merged, {
        ...taken,
        ...Object.fromEntries(variables.map((item) => [item.name, item.value])),
      });
      // A secret an MCP server refers to is available to MCP, and only to it
      // unless the same name is also an ordinary variable.
      for (const [name, value] of Object.entries(secrets)) {
        const at = variables.find((item) => item.name === name);
        if (at) {
          at.availableTo = ["setup", "agent", "mcp"];
          if (!at.value) Object.assign(at, { value, secret: true });
        } else
          variables.push({ name, value, secret: true, availableTo: ["mcp"] });
      }
      return { variables, servers };
    };

    handle("projects:import-local", async (id, cwd) => {
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      if (typeof cwd !== "string" || !path.isAbsolute(cwd))
        throw new Error("Choose an absolute project folder.");
      const remote = await execFileAsync(
        "git",
        ["remote", "get-url", "origin"],
        {
          cwd,
          timeout: 5000,
          maxBuffer: 10000,
        },
      )
        .then(({ stdout }) => stdout.trim())
        .catch(() => "");
      if (
        !remote ||
        normalizeRemote(remote) !== normalizeRemote(project.git?.url)
      )
        throw new Error("This folder is not a checkout of this project.");
      const found = await collect({
        endpoint: "local",
        root: cwd,
        local: true,
        taken: await takenValues(project),
      });
      return projects.mergeImport(id, found);
    });

    handle("projects:import-mcp-text", async (id, text) => {
      const project = await projects.get(id);
      if (!project) throw new Error("Unknown project.");
      let parsed;
      try {
        parsed = JSON.parse(text);
      } catch {
        throw new Error("Could not read that .mcp.json file.");
      }
      const servers = parsed?.mcpServers;
      if (!servers || typeof servers !== "object" || Array.isArray(servers))
        throw new Error(
          "The .mcp.json file must contain an mcpServers object.",
        );
      const cleaned = secretizeMcpServers(servers, await takenValues(project));
      return projects.mergeImport(id, {
        servers: cleaned.servers,
        variables: Object.entries(cleaned.secrets).map(([name, value]) => ({
          name,
          value,
          secret: true,
          availableTo: ["mcp"],
        })),
      });
    });

    // A new project has no id yet: what a source holds is kept here under a
    // token until the project is created, then moved into its store.
    handle("projects:scan-source", async (input) => {
      const { endpoint, root, local, example, mcp } = input || {};
      let repoMcp = {};
      try {
        repoMcp = JSON.parse(mcp || "{}").mcpServers || {};
      } catch {}
      const { variables, servers } = await collect({
        endpoint: typeof endpoint === "string" ? endpoint : undefined,
        root: typeof root === "string" ? root : undefined,
        local: !!local,
        example: typeof example === "string" ? example : undefined,
        repoMcp,
        taken: {},
      });
      const now = Date.now();
      for (const [key, entry] of pendingImports)
        if (now - entry.at > 30 * 60 * 1000) pendingImports.delete(key);
      const token = randomUUID();
      pendingImports.set(token, {
        at: now,
        values: Object.fromEntries(
          variables
            .filter((item) => item.secret && item.value)
            .map((item) => [item.name, item.value]),
        ),
      });
      // The install command its lock file implies, when the source has one.
      let install = "";
      if (endpoint && root)
        for (const [file, command] of LOCK_FILES) {
          const found = await connections()
            .inspect(endpoint, { operation: "read", root, path: file })
            .catch(() => null);
          if (found) {
            install = command;
            break;
          }
        }
      return {
        token,
        servers,
        install,
        variables: variables.map(({ name, secret, value, availableTo }) => ({
          name,
          secret,
          availableTo,
          // A secret's value stays here; only its presence is reported.
          ...(secret ? { held: !!value } : { value }),
        })),
      };
    });
    handle("projects:resolve", (remote) => projects.resolve(remote));
  }

  async function disconnectEndpoint(endpoint) {
    for (const pending of terminalPending.values()) {
      if (pending.endpoint === endpoint) pending.cancelled = true;
    }
    for (const terminal of terminals.values()) {
      if (terminal.endpoint === endpoint && !terminal.exited)
        terminal.proc.kill();
    }
    await connections().disconnect(endpoint);
    if (endpoint?.startsWith("ssh:"))
      await connections().setAutoConnect(endpoint, false);
  }

  handle("connections-list", () => connections().list());
  handle("connections-save", (profile) => connections().save(profile));
  handle("connections-set-hidden", (endpoint, hidden) =>
    connections().setHidden(endpoint, hidden),
  );
  handle("connections-delete", async (endpoint) => {
    await disconnectEndpoint(endpoint);
    await connections().delete(endpoint);
  });
  handle("connections-connect", async (endpoint) => {
    await connections().socket(endpoint);
    if (endpoint?.startsWith("ssh:")) {
      await connections().exec(endpoint, 'mkdir -p "$HOME/sushiai"\n');
      await connections().setAutoConnect(endpoint, true);
    }
    return { connected: true };
  });
  handle("connections-disconnect", disconnectEndpoint);
  handle("connections-forward", async (endpoint, url) => {
    const parsed = new URL(url);
    if (
      !["http:", "https:"].includes(parsed.protocol) ||
      !["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname)
    )
      throw new Error("Forwarding supports only a remote localhost URL.");
    const local = await connections().forward(
      endpoint,
      Number(parsed.port) || (parsed.protocol === "https:" ? 443 : 80),
    );
    parsed.hostname = "127.0.0.1";
    parsed.port = String(local);
    return parsed.href;
  });

  handle("project-inspect", (endpoint, options) => {
    if (
      ![
        "list",
        "read",
        "write",
        "git",
        "diff",
        "home",
        "log",
        "commit",
        "branches",
        "git_overview",
        "git_remote",
        "checkout",
      ].includes(options?.operation)
    )
      throw new Error("Unknown project operation");
    return connections().inspect(endpoint, options);
  });

  handle("project-preview", async (endpoint, root, file) => {
    await connections().inspect(endpoint, {
      operation: "read",
      root,
      path: file,
    });
    return getPreview().grant(endpoint, root, file);
  });

  handle("worktree-create", (cwd, branch) => createWorktree(cwd, branch));
}

module.exports = { registerProjectIpc };
