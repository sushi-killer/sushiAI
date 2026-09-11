const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");
const { ClaudePlugins } = require("../electron/claude-plugins.cjs");

async function remoteCall(home, input) {
  const source = await fs.readFile(
    path.join(__dirname, "../electron/remote-files.py"),
    "utf8",
  );
  return new Promise((resolve, reject) => {
    const child = spawn("/usr/bin/python3", ["-c", source], {
      env: { ...process.env, HOME: home },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) => {
      if (code) return reject(new Error(stderr || `python exited (${code})`));
      try {
        const result = JSON.parse(stdout);
        if (result.error) throw new Error(result.error);
        resolve(result.result);
      } catch (error) {
        reject(error);
      }
    });
    child.stdin.end(JSON.stringify(input));
  });
}

async function fixture() {
  const home = await fs.mkdtemp("/tmp/sushiai-claude-home-");
  const cwd = await fs.mkdtemp("/tmp/sushiai-claude-project-");
  await fs.writeFile(
    path.join(cwd, ".mcp.json"),
    JSON.stringify({
      mcpServers: {
        projectTools: { command: "node", args: ["server.js"] },
        duplicate: { command: "node", args: ["project.js"] },
      },
    }),
  );
  await fs.writeFile(
    path.join(home, ".claude.json"),
    JSON.stringify({
      oauthAccount: { token: "do-not-return" },
      mcpServers: {
        userTools: { command: "node", env: { SECRET: "do-not-return" } },
        duplicate: { command: "node", args: ["user.js"] },
      },
      projects: {
        [cwd]: {
          mcpServers: {
            localTools: { command: "node", args: ["local.js"] },
            duplicate: { command: "node", args: ["local.js"] },
          },
          disabledMcpServers: ["claude.ai Calendar", "userTools"],
          disabledMcpjsonServers: ["projectTools"],
        },
      },
    }),
  );
  return {
    home,
    cwd,
    claude: new ClaudeMcp({ home }),
    async readConfig() {
      return JSON.parse(
        await fs.readFile(path.join(home, ".claude.json"), "utf8"),
      );
    },
    async cleanup() {
      await Promise.all([
        fs.rm(home, { recursive: true, force: true }),
        fs.rm(cwd, { recursive: true, force: true }),
      ]);
    },
  };
}

test("lists project, local, user and saved Claude MCP choices without secrets", async () => {
  const f = await fixture();
  try {
    const result = await f.claude.list(f.cwd);
    assert.deepEqual(
      result.servers.map((server) => [
        server.name,
        server.source,
        server.disabled,
      ]),
      [
        ["claude.ai Calendar", "saved", true],
        ["duplicate", "local", false],
        ["localTools", "local", false],
        ["projectTools", "project", true],
        ["userTools", "user", true],
      ],
    );
    assert.equal(JSON.stringify(result).includes("do-not-return"), false);
  } finally {
    await f.cleanup();
  }
});

test("toggles project-scoped servers through disabledMcpjsonServers and preserves config", async () => {
  const f = await fixture();
  try {
    await f.claude.toggle({
      cwd: f.cwd,
      name: "projectTools",
      source: "project",
      disabled: true,
    });
    let config = await f.readConfig();
    assert.deepEqual(config.projects[f.cwd].disabledMcpjsonServers, [
      "projectTools",
    ]);
    assert.deepEqual(config.projects[f.cwd].mcpServers.duplicate.args, [
      "local.js",
    ]);
    assert.equal(config.oauthAccount.token, "do-not-return");

    await f.claude.toggle({
      cwd: f.cwd,
      name: "projectTools",
      source: "project",
      disabled: false,
    });
    config = await f.readConfig();
    assert.deepEqual(config.projects[f.cwd].disabledMcpjsonServers, []);
    assert.deepEqual(config.projects[f.cwd].disabledMcpServers, [
      "claude.ai Calendar",
      "userTools",
    ]);
  } finally {
    await f.cleanup();
  }
});

test("toggles user and saved official choices through disabledMcpServers", async () => {
  const f = await fixture();
  try {
    await f.claude.toggle({ cwd: f.cwd, name: "userTools", disabled: false });
    await f.claude.toggle({
      cwd: f.cwd,
      name: "claude.ai Calendar",
      disabled: false,
    });
    const config = await f.readConfig();
    assert.deepEqual(config.projects[f.cwd].disabledMcpServers, []);

    await f.claude.toggle({
      cwd: f.cwd,
      name: "claude.ai Drive",
      disabled: true,
    });
    const next = await f.claude.list(f.cwd);
    assert.deepEqual(
      next.servers.find((server) => server.name === "claude.ai Drive"),
      {
        name: "claude.ai Drive",
        source: "saved",
        sourceLabel: "Claude.ai connector",
        disabled: true,
      },
    );
  } finally {
    await f.cleanup();
  }
});

test("rejects non-project paths and malformed MCP state", async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-claude-home-");
  const claude = new ClaudeMcp({ home });
  try {
    await assert.rejects(claude.list("relative"), /local project folder/);
    await assert.rejects(
      claude.toggle({ cwd: "/tmp", name: "", disabled: true }),
      /MCP server name/,
    );
    await fs.writeFile(path.join(home, ".claude.json"), "not-json");
    await assert.rejects(claude.list("/tmp"), /Could not read Claude Code/);
  } finally {
    await fs.rm(home, { recursive: true, force: true });
  }
});

test("lists Claude plugins and writes a per-project local override", async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-claude-plugin-home-");
  const cwd = await fs.mkdtemp("/tmp/sushiai-claude-plugin-project-");
  try {
    await fs.mkdir(path.join(home, ".claude/plugins"), { recursive: true });
    await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
    await fs.writeFile(
      path.join(home, ".claude/settings.json"),
      JSON.stringify({
        enabledPlugins: {
          "user-tools@official": true,
          "shared-tools@official": false,
        },
      }),
    );
    await fs.writeFile(
      path.join(cwd, ".claude/settings.json"),
      JSON.stringify({
        enabledPlugins: { "project-tools@team": true },
        permissions: { allow: ["Read"] },
      }),
    );
    await fs.writeFile(
      path.join(home, ".claude/plugins/installed_plugins.json"),
      JSON.stringify({
        plugins: {
          "user-tools@official": [],
          "installed-only@community": [],
        },
      }),
    );
    const plugins = new ClaudePlugins({ home });
    const listed = await plugins.list(cwd);
    assert.deepEqual(
      listed.plugins.map((plugin) => [plugin.name, plugin.disabled]),
      [
        ["installed-only@community", true],
        ["project-tools@team", false],
        ["shared-tools@official", true],
        ["user-tools@official", false],
      ],
    );
    await plugins.toggle({
      cwd,
      name: "project-tools@team",
      disabled: true,
    });
    const local = JSON.parse(
      await fs.readFile(path.join(cwd, ".claude/settings.local.json"), "utf8"),
    );
    assert.equal(local.enabledPlugins["project-tools@team"], false);
    const project = JSON.parse(
      await fs.readFile(path.join(cwd, ".claude/settings.json"), "utf8"),
    );
    assert.equal(project.permissions.allow[0], "Read");
  } finally {
    await Promise.all([
      fs.rm(home, { recursive: true, force: true }),
      fs.rm(cwd, { recursive: true, force: true }),
    ]);
  }
});

test("remote protocol reads and toggles MCP state on the selected SSH host", async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-remote-claude-home-");
  const cwd = await fs.mkdtemp("/tmp/sushiai-remote-claude-project-");
  try {
    await fs.mkdir(path.join(home, ".claude/plugins"), { recursive: true });
    await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
    await fs.writeFile(
      path.join(cwd, ".mcp.json"),
      JSON.stringify({
        mcpServers: { remoteProject: { command: "node", args: ["server.js"] } },
      }),
    );
    await fs.writeFile(
      path.join(home, ".claude.json"),
      JSON.stringify({
        oauthAccount: { token: "remote-secret" },
        mcpServers: { remoteUser: { env: { TOKEN: "remote-secret" } } },
        projects: { [cwd]: { disabledMcpServers: ["remoteUser"] } },
      }),
    );
    await fs.writeFile(
      path.join(home, ".claude/settings.json"),
      JSON.stringify({ enabledPlugins: { "remote-tools@official": true } }),
    );
    await fs.writeFile(
      path.join(cwd, ".claude/settings.json"),
      JSON.stringify({ enabledPlugins: { "project-tools@team": true } }),
    );
    await fs.writeFile(
      path.join(home, ".claude/plugins/installed_plugins.json"),
      JSON.stringify({ plugins: { "remote-tools@official": [] } }),
    );
    const listed = await remoteCall(home, {
      operation: "claude_mcp",
      action: "list",
      cwd,
    });
    assert.deepEqual(
      listed.servers.map((server) => [server.name, server.disabled]),
      [
        ["remoteProject", false],
        ["remoteUser", true],
      ],
    );
    assert.equal(JSON.stringify(listed).includes("remote-secret"), false);
    await remoteCall(home, {
      operation: "claude_mcp",
      action: "toggle",
      cwd,
      name: "remoteProject",
      source: "project",
      disabled: true,
    });
    const saved = JSON.parse(
      await fs.readFile(path.join(home, ".claude.json"), "utf8"),
    );
    assert.deepEqual(saved.projects[cwd].disabledMcpjsonServers, [
      "remoteProject",
    ]);
    assert.equal(saved.oauthAccount.token, "remote-secret");
    const plugins = await remoteCall(home, {
      operation: "claude_plugins",
      action: "list",
      cwd,
    });
    assert.deepEqual(
      plugins.plugins.map((plugin) => [plugin.name, plugin.disabled]),
      [
        ["project-tools@team", false],
        ["remote-tools@official", false],
      ],
    );
    await fs.rm(path.join(cwd, ".claude"), { recursive: true, force: true });
    await remoteCall(home, {
      operation: "claude_plugins",
      action: "toggle",
      cwd,
      name: "project-tools@team",
      disabled: true,
    });
    const remoteLocal = JSON.parse(
      await fs.readFile(path.join(cwd, ".claude/settings.local.json"), "utf8"),
    );
    assert.equal(remoteLocal.enabledPlugins["project-tools@team"], false);
  } finally {
    await Promise.all([
      fs.rm(home, { recursive: true, force: true }),
      fs.rm(cwd, { recursive: true, force: true }),
    ]);
  }
});
