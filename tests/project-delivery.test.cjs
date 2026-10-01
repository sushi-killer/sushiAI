// What a host gets and when: a host the owner added gets a project's values
// with no question asked; one switched off for the project gets none; the
// picker's flow and project list; MCP usage counted from transcripts.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  makeHost,
  makeStore,
  registerHandlers,
} = require("./helpers/fake-host.cjs");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");

const identity = {
  GIT_AUTHOR_NAME: "Dev",
  GIT_AUTHOR_EMAIL: "dev@example.invalid",
  GIT_COMMITTER_NAME: "Dev",
  GIT_COMMITTER_EMAIL: "dev@example.invalid",
};

async function origin(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "delivery-origin-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  await fs.writeFile(path.join(dir, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", dir, "add", "."]);
  execFileSync("git", ["-C", dir, "commit", "-qm", "init"], {
    env: { ...process.env, ...identity },
  });
  return dir;
}

async function setup(t) {
  const host = await makeHost(t, {
    bin: {
      npm: '#!/bin/sh\nprintf "[%s][%s]" "$NPM_TOKEN" "$GIT_TOKEN" > "$HOME/npm-saw"\n',
      checker: '#!/bin/sh\necho ran > "$HOME/check-ran"\n',
    },
  });
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "My App",
    git: { url: await origin(t), defaultBranch: "main" },
    setup: { install: "npm ci", check: "checker" },
    env: [
      { name: "NPM_TOKEN", secret: true, availableTo: ["setup"] },
      { name: "GIT_TOKEN", secret: true, availableTo: ["setup"] },
    ],
  });
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  await projects.setSecret(project.id, "GIT_TOKEN", "invented-git-token");
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  return { host, projects, project, call };
}

const exists = (file) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

test("a host switched off for the project is prepared with nothing sent", async (t) => {
  const { host, projects, project, call } = await setup(t);
  await projects.setHostWithheld(project.id, host.endpoint, true);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  // Cloned and installed; the install saw no variable; the check (which needs
  // them) did not run.
  assert.equal(
    await fs.readFile(path.join(host.home, "npm-saw"), "utf8"),
    "[][]",
  );
  assert.equal(await exists(path.join(host.home, "check-ran")), false);
  assert.deepEqual(
    result.steps.map((step) => step.id),
    ["clone", "install"],
  );
  const argv = await fs.readFile(host.log, "utf8");
  for (const value of ["invented-npm-token", "invented-git-token"])
    assert.equal(argv.includes(value), false);
});

test("a host that was just added gets the values with no question asked", async (t) => {
  const { host, project, call } = await setup(t);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  // The install saw its setup variable (not the clone token) and the check ran.
  assert.equal(
    await fs.readFile(path.join(host.home, "npm-saw"), "utf8"),
    "[invented-npm-token][]",
  );
  assert.equal(await exists(path.join(host.home, "check-ran")), true);
});

function fakeBridge(log, { prepare } = {}) {
  return {
    projectHostPrepare: async (...args) => {
      log.push(["prepare", ...args]);
      return (
        prepare?.(...args) ?? {
          ok: true,
          path: "~/sushiai/app",
          message: "Cloned",
          steps: [],
        }
      );
    },
  };
}

test("the picker's flow retries as asked, and never starts a session nobody waits for", async () => {
  const { prepareAndStart } = await import("../src/projectDeliver.ts");
  const base = {
    projectId: "p",
    endpoint: "ssh:lab",
    useHostLogin: false,
    isGone: () => false,
  };
  let log = [];
  const started = [];
  const failing = fakeBridge(log, {
    prepare: () => ({ ok: false, stage: "clone", status: 403, message: "403" }),
  });
  for (const useHostLogin of [false, true]) {
    const outcome = await prepareAndStart({
      ...base,
      bridge: failing,
      useHostLogin,
      start: async (path) => started.push(path) > 0,
    });
    assert.equal(outcome.kind, "failed");
  }
  // The retry that uses the host's own git login says so.
  assert.deepEqual(
    log.map((entry) => entry[3]),
    [false, true],
  );
  assert.deepEqual(started, []);
  // A prepare that works starts the session in the prepared folder.
  log = [];
  let outcome = await prepareAndStart({
    ...base,
    bridge: fakeBridge(log),
    start: async (path) => started.push(path) > 0,
  });
  assert.equal(outcome.kind, "started");
  assert.deepEqual(started, ["~/sushiai/app"]);
  // Walking away after the prepare: no session.
  started.length = 0;
  outcome = await prepareAndStart({
    ...base,
    isGone: () => true,
    bridge: fakeBridge([]),
    start: async (path) => started.push(path) > 0,
  });
  assert.equal(outcome.kind, "cancelled");
  assert.deepEqual(started, []);
  // A session that cannot start is its own outcome.
  outcome = await prepareAndStart({
    ...base,
    bridge: fakeBridge([]),
    start: async () => false,
  });
  assert.equal(outcome.kind, "unstarted");
  assert.equal(outcome.path, "~/sushiai/app");
  // A prepare that throws is a failure, not a hang.
  outcome = await prepareAndStart({
    ...base,
    bridge: {
      projectHostPrepare: async () => {
        throw new Error("ssh dropped");
      },
    },
    start: async () => true,
  });
  assert.equal(outcome.kind, "failed");
  assert.match(outcome.failure.message, /ssh dropped/);
});

test("failure screens say what was and was not sent", async () => {
  const { sentLine, advice, cloneFailure } =
    await import("../src/orchestrator/prepareCopy.ts");
  const input = {
    hostName: "devbox",
    repo: "acme/app",
    token: "GITHUB_TOKEN",
    noSecrets: false,
    status: 403,
    message: "403",
  };
  // Clone fails, trusted: only the clone token was in play; no install ran.
  const cloneFailed = { ...input, stage: "clone" };
  assert.equal(
    sentLine(cloneFailed),
    "GITHUB_TOKEN was used for the clone. The install did not run, so no other value was used.",
  );
  assert.equal(
    sentLine({ ...cloneFailed, token: null }),
    "No value was used; the install did not run.",
  );
  // Install fails, trusted: secrets by name, everything else counted, and a
  // plain variable is never listed as a secret.
  const installFailed = {
    ...input,
    status: undefined,
    stage: "setup",
    setupSecrets: ["NPM_TOKEN", "ACME_API_KEY"],
    setupOthers: 2,
  };
  assert.equal(
    sentLine(installFailed),
    "GITHUB_TOKEN was used for the clone, and the install ran with NPM_TOKEN, ACME_API_KEY, 2 other variables.",
  );
  assert.doesNotMatch(sentLine(installFailed), /NODE_ENV|DATABASE_URL/);
  assert.equal(
    sentLine({
      ...installFailed,
      token: null,
      setupSecrets: [],
      setupOthers: 0,
    }),
    "The install ran with no project values.",
  );
  // A host switched off for the project: nothing was sent, whatever the stage.
  for (const stage of ["clone", "setup"])
    assert.equal(
      sentLine({ ...input, noSecrets: true, stage }),
      "No value was sent to devbox; the clone used its own git login.",
    );
  assert.match(
    sentLine({ ...input, noSecrets: true }),
    /No value was sent to devbox/,
  );
  // A prepare without secrets never advises about a token it did not use.
  const without = advice({ ...input, noSecrets: true });
  assert.match(without, /devbox’s git login cannot read acme\/app/);
  assert.match(without, /Don’t send secrets to this host/);
  assert.doesNotMatch(without, /Give the token read access/);
  assert.match(advice(input), /Give the token read access to acme\/app/);
  assert.equal(
    cloneFailure({ ...input, noSecrets: true }),
    "git clone failed: devbox’s git login has no access to acme/app (403)",
  );
  assert.equal(cloneFailure({ ...input, status: 500 }), null);
});

test("a launch lands in the project the picker was switched to", async () => {
  const { launchTarget } = await import("../src/app/sessionHosts.ts");
  assert.equal(launchTarget(0, "w1", "w1", "w1"), undefined);
  assert.equal(launchTarget(2, "w2", "w1", "w1"), "w2");
  // Switched to another project with no merge group: its workspace, never
  // the one the picker opened on.
  assert.equal(launchTarget(0, "w9", "w9", "w1"), "w9");
});

test("a kept checkout is said plainly", () => {
  const { pullMessage } = require("../electron/project-git.cjs");
  assert.equal(
    pullMessage("skipped:fetch-failed"),
    "Kept the checkout · fetch failed",
  );
  assert.equal(
    pullMessage("skipped:local-changes"),
    "Kept the checkout · local changes",
  );
  assert.equal(pullMessage("current"), "Checkout up to date");
});

test("a host switched off for the project runs git with no token", async (t) => {
  const host = await makeHost(t, {
    bin: {
      npm: "#!/bin/sh\nexit 0\n",
      git: '#!/bin/sh\nprintf "[%s][%s]\\n" "$GIT_ASKPASS" "$SUSHIAI_GIT_TOKEN" >> "$HOME/git-saw"\nexec REAL_GIT "$@"\n',
    },
  });
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "My App",
    git: { url: await origin(t), defaultBranch: "main" },
    setup: { install: "npm ci", check: "" },
    env: [{ name: "GIT_TOKEN", secret: true, availableTo: ["setup"] }],
  });
  await projects.setSecret(project.id, "GIT_TOKEN", "invented-git-token");
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  await projects.setHostWithheld(project.id, host.endpoint, true);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  const saw = await fs.readFile(path.join(host.home, "git-saw"), "utf8");
  assert.match(saw, /^(\[\]\[\]\n)+$/);
});

test("the picker lists the projects that have a workspace open, and targets that workspace", async () => {
  const { projectChoices } = await import("../src/app/sessionHosts.ts");
  const projects = [
    { id: "a", name: "Alpha", git: { url: "git@example.test:acme/alpha.git" } },
    { id: "b", name: "Beta", git: { url: "https://example.test/acme/beta" } },
    { id: "c", name: "Gamma", git: { url: "" } },
  ];
  const workspaces = [{ id: "w1" }, { id: "w2" }];
  const choices = projectChoices(projects, workspaces, {
    w1: { remote: "https://example.test/acme/alpha" },
    w2: { remote: "git@example.test:acme/other.git" },
  });
  assert.deepEqual(
    choices.map((choice) => [choice.project.name, choice.workspace.id]),
    [["Alpha", "w1"]],
  );
  // A folder with no remote is found by the folder it was attached to.
  const notes = {
    id: "n",
    name: "Notes",
    git: { url: "" },
    folders: [{ endpoint: "local", cwd: "/work/notes" }],
  };
  const byFolder = projectChoices(
    [notes],
    [
      { id: "w3", cwd: "/work/other" },
      { id: "w4", cwd: "/work/notes" },
      { id: "w5", cwd: "/work/notes", connection: "ssh:lab" },
    ],
    {},
  );
  assert.deepEqual(
    byFolder.map((choice) => choice.workspace.id),
    ["w4"],
  );
});

test("MCP usage is counted from this project's transcripts in the last 30 days", async (t) => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "usage-home-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const project = path.join(home, "work", "app");
  await fs.mkdir(project, { recursive: true });
  const folder = path.join(
    home,
    ".claude",
    "projects",
    project.replace(/[^A-Za-z0-9]/g, "-"),
  );
  await fs.mkdir(folder, { recursive: true });
  const now = Date.parse("2026-10-01T12:00:00Z");
  const day = 86400000;
  const line = (ago, ...tools) =>
    JSON.stringify({
      timestamp: new Date(now - ago * day).toISOString(),
      message: {
        content: tools.map((name) => ({ type: "tool_use", id: "x", name })),
      },
    });
  await fs.writeFile(
    path.join(folder, "session.jsonl"),
    [
      line(1, "mcp__context7__resolve", "mcp__context7__query"),
      line(3, "mcp__context7__query", "mcp__plugin_superpowers_memory__find"),
      line(40, "mcp__context7__query"), // too old
      line(2, "Bash"),
      line(2, "mcp__plugin_superpowers_search__run"),
      "not json mcp__ tool_use",
    ].join("\n"),
  );
  const usage = await new ClaudeMcp({ home }).usage(project, now);
  assert.deepEqual(usage.servers, { context7: 3 });
  assert.deepEqual(usage.plugins, {
    superpowers: { uses: 2, servers: ["memory", "search"] },
  });
  // Nothing recorded is nothing claimed.
  assert.deepEqual(await new ClaudeMcp({ home }).usage(home, now), {
    servers: {},
    plugins: {},
  });
  const { usesText, pluginLine } = await import("../src/projectMcp.ts");
  assert.equal(usesText(0), "");
  assert.equal(usesText(1), "1 use in 30 days");
  assert.equal(usesText(14), "14 uses in 30 days");
  assert.equal(
    pluginLine({ skills: true, servers: 2, uses: 31 }),
    "Claude plugin · skills and 2 servers · 31 uses in 30 days",
  );
  assert.equal(
    pluginLine({ skills: false, servers: 0, uses: 0 }),
    "Claude plugin",
  );
});

test("the MCP count line counts servers that use secrets, not the secrets", async () => {
  const { serversUsingSecrets, mcpCountLine } =
    await import("../src/projectMcp.ts");
  const variables = [
    { name: "TOKEN_A", secret: true },
    { name: "TOKEN_B", secret: true },
    { name: "REGION", secret: false },
  ];
  const servers = {
    // One server, two secret headers.
    docs: {
      url: "https://example.test/mcp",
      headers: { A: "${TOKEN_A}", B: "Bearer ${TOKEN_B}" },
    },
    plain: { command: "npx", args: ["x"], vars: { R: "${REGION}" } },
  };
  assert.equal(serversUsingSecrets(servers, variables), 1);
  assert.equal(
    mcpCountLine(2, serversUsingSecrets(servers, variables)),
    "2 servers · 1 uses a secret",
  );
  assert.equal(mcpCountLine(1, 0), "1 server · no secrets");
});
