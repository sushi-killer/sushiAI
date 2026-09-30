const fs = require("node:fs/promises");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execFileAsync = promisify(execFile);
const { createWorktree } = require("../worktree.cjs");
const {
  hostProbeScript,
  readiness,
  prepareScript,
} = require("../project-hosts.cjs");

function registerProjectIpc({
  handle,
  getConnections,
  getPreview,
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
      const { stdout } = await execFileAsync(
        "git",
        ["ls-remote", "--symref", remote, "HEAD"],
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
        for (const file of [
          "package-lock.json",
          "pnpm-lock.yaml",
          "yarn.lock",
          "bun.lock",
          "bun.lockb",
          "Cargo.lock",
          "uv.lock",
          "poetry.lock",
          "Pipfile.lock",
          "Gemfile.lock",
          "composer.lock",
          "go.sum",
        ]) {
          try {
            await fs.access(path.join(directory, file));
            lockFile = file;
            break;
          } catch {}
        }
        return { branch, envExample, mcp, lockFile };
      } finally {
        await fs.rm(directory, { recursive: true, force: true });
      }
    });
    handle("projects:local-create", async ({ url, cwd, branch, empty }) => {
      if (typeof cwd !== "string" || !path.isAbsolute(cwd))
        throw new Error("Choose an absolute project folder.");
      if (empty) {
        await fs.mkdir(cwd, { recursive: true });
        await execFileAsync("git", ["init", cwd]);
      } else {
        if (typeof url !== "string" || !url.trim())
          throw new Error("Enter a git URL.");
        await fs.mkdir(path.dirname(cwd), { recursive: true });
        await execFileAsync("git", ["clone", "--", url.trim(), cwd], {
          timeout: 15 * 60 * 1000,
        });
        if (branch && branch !== "main")
          await execFileAsync("git", ["-C", cwd, "checkout", branch]);
      }
      return { cwd };
    });
    handle("projects:list", () => projects.list());
    handle("projects:get", (id) => projects.get(id));
    handle("projects:upsert", (project) => projects.upsert(project));
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
        hostProbeScript(cwd, project.name),
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
      const tokenEntry = project.env.find(
        (entry) => entry.name === "GIT_TOKEN",
      );
      const token =
        useHostLogin || !tokenEntry
          ? ""
          : await projects.secretForHost(id, "GIT_TOKEN", host);
      const prepared = prepareScript(project, token);
      try {
        const output = await connections().exec(host, prepared.script, {
          input: prepared.input,
          timeout: 15 * 60 * 1000,
        });
        const path =
          output.match(/^SUSHIAI_PREPARED=(.+)$/m)?.[1] || prepared.path;
        return { ok: true, path, output };
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const status = message.match(/\b(401|403)\b/)?.[1];
        return {
          ok: false,
          stage: message.includes("SUSHIAI_PREPARE_STAGE=clone")
            ? "clone"
            : "setup",
          status: status ? Number(status) : undefined,
          message,
        };
      }
    });
    handle("projects:env:import-review", (id, entries) =>
      projects.reviewEnvImport(id, entries),
    );
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
