// Drives the real project IPC handlers, store and prepare script: option
// injection through a git URL, a failure that stays correctly named under
// noisy output, folders that are not this project's checkout, and values that
// must neither be overwritten nor stay literal.
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
const { prepareScript } = require("../electron/project-hosts.cjs");
const { localCreate } = require("../electron/project-git.cjs");
const {
  isSecret,
  secretizeMcpServers,
} = require("../electron/project-import.cjs");

const identity = {
  GIT_AUTHOR_NAME: "Dev",
  GIT_AUTHOR_EMAIL: "dev@example.invalid",
  GIT_COMMITTER_NAME: "Dev",
  GIT_COMMITTER_EMAIL: "dev@example.invalid",
};

async function origin(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "hardening-origin-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  await fs.writeFile(path.join(dir, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", dir, "add", "."]);
  execFileSync("git", ["-C", dir, "commit", "-qm", "init"], {
    env: { ...process.env, ...identity },
  });
  return dir;
}

async function setup(t, { bin, env = [], connections } = {}) {
  const host = await makeHost(t, { bin });
  const { projects } = await makeStore(t);
  const url = await origin(t);
  const project = await projects.upsert({
    name: "My App",
    git: { url, defaultBranch: "main" },
    setup: { install: "npm ci", check: "" },
    env,
  });
  const call = registerHandlers({
    projects,
    connections: connections ? connections(host) : host.connections,
    claudeMcp: {},
  });
  return { host, projects, project, call };
}

const exists = (file) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

test("a git URL that is an option never reaches git as one", async (t) => {
  const marker = path.join(
    os.tmpdir(),
    `injected-${process.pid}-${Date.now()}`,
  );
  t.after(() => fs.rm(marker, { force: true }));
  const url = `--upload-pack=touch ${marker};`;
  const { projects } = await makeStore(t);
  const call = registerHandlers({ projects, connections: {}, claudeMcp: {} });
  await assert.rejects(call("projects:source-inspect", url), /dash/);
  await assert.rejects(projects.upsert({ name: "X", git: { url } }), /dash/);
  await assert.rejects(
    localCreate({ url, cwd: path.join(os.tmpdir(), `never-${process.pid}`) }),
    /dash/,
  );
  assert.throws(
    () => prepareScript({ name: "X", git: { url, defaultBranch: "main" } }, ""),
    /dash/,
  );
  assert.equal(await exists(marker), false);
});

test("project inspection exposes only the explicit worktree cleanup operations", async (t) => {
  const { projects } = await makeStore(t);
  const received = [];
  const call = registerHandlers({
    projects,
    connections: {
      inspect: async (endpoint, options) => {
        received.push({ endpoint, options });
        return { state: "MERGED", mergedAt: "2026-10-01T00:00:00Z" };
      },
    },
    claudeMcp: {},
  });

  assert.deepEqual(
    await call("project-inspect", "ssh:devbox", {
      operation: "git_pr_status",
      root: "/repo-feature",
      branch: "feature/task",
    }),
    { state: "MERGED", mergedAt: "2026-10-01T00:00:00Z" },
  );
  assert.deepEqual(received, [
    {
      endpoint: "ssh:devbox",
      options: {
        operation: "git_pr_status",
        root: "/repo-feature",
        branch: "feature/task",
      },
    },
  ]);
  assert.throws(
    () =>
      call("project-inspect", undefined, {
        operation: "git_worktree_remove;anything",
        root: "/repo-feature",
      }),
    /Unknown project operation/,
  );
});

test("a noisy install that fails with a 403 is still an install failure", async (t) => {
  const { host, project, call } = await setup(t, {
    bin: {
      npm: '#!/bin/sh\ni=0\nwhile [ $i -lt 400 ]; do echo "npm WARN deprecated package-$i: a long and noisy warning line" >&2; i=$((i+1)); done\necho "npm ERR! 403 Forbidden" >&2\nexit 1\n',
    },
  });
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.equal(result.stage, "setup");
  assert.equal(result.status, undefined);
  assert.equal(
    result.steps.find((step) => step.id === "install").state,
    "failed",
  );
});

test("a folder at the standard path that is not this project's checkout is refused", async (t) => {
  const { host, project, call } = await setup(t, {
    bin: { npm: '#!/bin/sh\necho ran > "$HOME/npm-saw"\n' },
  });
  const folder = path.join(host.home, "sushiai", "my-app");
  // Not a git repository at all.
  await fs.mkdir(folder, { recursive: true });
  await fs.writeFile(path.join(folder, "notes.txt"), "mine\n");
  let result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.match(result.message, /not a git checkout/);
  assert.deepEqual(await fs.readdir(folder), ["notes.txt"]);
  // A repository with no origin.
  await fs.rm(folder, { recursive: true });
  await fs.mkdir(folder, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", folder]);
  result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.match(result.message, /without an origin/);
  assert.equal(await exists(path.join(host.home, "npm-saw")), false);
});

test("a probe that cannot run sends nothing, and the script itself will not move a clone into an existing folder", async (t) => {
  const broken = await setup(t, {
    connections: (host) => ({
      exec: async (endpoint, command, options) => {
        if (command.includes("ABSENT")) throw new Error("ssh unavailable");
        return host.connections.exec(endpoint, command, options);
      },
    }),
  });
  let result = await broken.call(
    "projects:host:prepare",
    broken.project.id,
    broken.host.endpoint,
  );
  assert.equal(result.ok, false);
  assert.match(result.message, /Could not look/);

  // A probe that wrongly said "absent" for a folder that exists.
  const lying = await setup(t, {
    connections: (host) => ({
      exec: async (endpoint, command, options) =>
        command.includes("ABSENT")
          ? "ABSENT\n"
          : host.connections.exec(endpoint, command, options),
    }),
  });
  const folder = path.join(lying.host.home, "sushiai", "my-app");
  await fs.mkdir(folder, { recursive: true });
  result = await lying.call(
    "projects:host:prepare",
    lying.project.id,
    lying.host.endpoint,
  );
  assert.equal(result.ok, false);
  assert.deepEqual(await fs.readdir(folder), []);
});

test("the git token is not among the variables the install sees", async (t) => {
  const { host, projects, project, call } = await setup(t, {
    bin: {
      npm: '#!/bin/sh\nprintf "[%s][%s]" "$GIT_TOKEN" "$NPM_TOKEN" > "$HOME/npm-saw"\n',
    },
    env: [
      { name: "GIT_TOKEN", secret: true, availableTo: ["setup"] },
      { name: "NPM_TOKEN", secret: true, availableTo: ["setup"] },
    ],
  });
  await projects.setSecret(project.id, "GIT_TOKEN", "invented-git-token");
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await fs.readFile(path.join(host.home, "npm-saw"), "utf8"),
    "[][invented-npm-token]",
  );
});

test("a stale copy of a project cannot overwrite what an import added", async (t) => {
  const { projects } = await makeStore(t);
  const created = await projects.upsert({ name: "App", git: { url: "x" } });
  const stale = await projects.get(created.id);
  await projects.mergeImport(created.id, {
    variables: [{ name: "API_TOKEN", secret: true, value: "invented-value" }],
    servers: { docs: { url: "https://docs.example.test/mcp" } },
  });
  const saved = await projects.upsert({ ...stale, name: "Renamed" });
  assert.equal(saved.name, "Renamed");
  assert.deepEqual(
    saved.env.map((entry) => entry.name),
    ["API_TOKEN"],
  );
  assert.deepEqual(Object.keys(saved.mcp.mcpServers), ["docs"]);
  assert.equal(
    await projects.secretFor(created.id, "API_TOKEN"),
    "invented-value",
  );
});

test("MCP credentials in args and URLs become references, however they are imported", async () => {
  const { servers, secrets } = secretizeMcpServers({
    db: {
      command: "npx",
      args: [
        "server-postgres",
        "postgresql://app:invented-pw@db.example.test/app",
        "--api-key=invented-key",
        "--token",
        "invented-flag-token",
        "--port=8080",
      ],
    },
    web: {
      url: "https://user:invented-url-pw@mcp.example.test/x?api_key=invented-q&q=1",
    },
  });
  const text = JSON.stringify(servers);
  for (const value of [
    "invented-pw",
    "invented-key",
    "invented-flag-token",
    "invented-url-pw",
    "invented-q",
  ])
    assert.equal(text.includes(value), false, `${value} stayed literal`);
  assert.equal(servers.db.args.at(-1), "--port=8080");
  assert.equal(servers.web.url.includes("q=1"), true);
  assert.deepEqual(
    Object.values(secrets).sort(),
    [
      "invented-flag-token",
      "invented-key",
      "invented-q",
      "invented-url-pw",
      "postgresql://app:invented-pw@db.example.test/app",
    ].sort(),
  );
});

test("the classifier knows credentials by their shape and by more names", () => {
  for (const [name, value] of [
    ["REDIS_URL", "redis://:invented@cache.example.test"],
    ["SLACK_WEBHOOK_URL", "https://example.test/hook"],
    ["STRIPE_SK", "sk_live_invented"],
    ["GH_PAT", "ghp_invented"],
    ["PLAIN", "github_pat_invented"],
    ["PLAIN", "xoxb-invented"],
    ["PLAIN", "AKIAINVENTED1234"],
    ["PLAIN", "https://hooks.slack.com/services/T0/B0/invented"],
  ])
    assert.equal(isSecret(name, value), true, `${name}=${value}`);
  for (const [name, value] of [
    ["KEYBOARD", "us"],
    ["COMPASS", "north"],
    ["PORT", "8080"],
    ["API_URL", "https://api.example.test"],
  ])
    assert.equal(isSecret(name, value), false, `${name}=${value}`);
});

test("a secret made plain loses its stored value, and no hint carries one back", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    env: [{ name: "CODE", secret: true }],
  });
  await projects.setSecret(project.id, "CODE", "invented-secret-value");
  const flipped = await projects.updateEnv(project.id, {
    set: [{ name: "CODE", secret: false }],
  });
  assert.equal(
    JSON.stringify(flipped).includes("invented-secret-value"),
    false,
  );
  assert.equal(await projects.secretFor(project.id, "CODE"), null);
  assert.equal(flipped.env[0].hasValue, false);
});

test("MCP servers typed by hand are stored as references with a secret variable", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({ name: "App", git: { url: "x" } });
  const call = registerHandlers({ projects, connections: {}, claudeMcp: {} });
  const updated = await call("projects:mcp:update", project.id, {
    set: {
      api: {
        command: "npx",
        env: { API_TOKEN: "invented-typed-token" },
        args: ["--password=invented-typed-pw"],
        url: undefined,
      },
    },
  });
  assert.equal(JSON.stringify(updated).includes("invented-typed"), false);
  assert.equal(
    updated.env.every((entry) => entry.secret),
    true,
  );
  assert.equal(
    await projects.secretFor(project.id, "API_TOKEN"),
    "invented-typed-token",
  );
  // The disabled list is set on its own, without touching the servers.
  const disabled = await call("projects:mcp:update", project.id, {
    disabled: ["api"],
  });
  assert.deepEqual(disabled.mcp.disabledMcpServers, ["api"]);
  assert.deepEqual(Object.keys(disabled.mcp.mcpServers), ["api"]);
});

// ---- final review round ----------------------------------------------------

test("a git token can be saved for a project that keeps none yet", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({ name: "App", git: { url: "x" } });
  const call = registerHandlers({ projects, connections: {}, claudeMcp: {} });
  const saved = await call(
    "projects:git-token:set",
    project.id,
    "invented-clone-token",
  );
  const entry = saved.env.find((item) => item.name === "GIT_TOKEN");
  assert.deepEqual([entry.secret, entry.availableTo], [true, ["setup"]]);
  assert.equal(
    await projects.secretFor(project.id, "GIT_TOKEN"),
    "invented-clone-token",
  );
  // A project that already keeps GITHUB_TOKEN uses that one.
  const other = await projects.upsert({
    name: "Other",
    env: [{ name: "GITHUB_TOKEN", secret: true }],
  });
  await call("projects:git-token:set", other.id, "invented-other-token");
  assert.equal(
    await projects.secretFor(other.id, "GITHUB_TOKEN"),
    "invented-other-token",
  );
  assert.equal(
    (await projects.get(other.id)).env.some(
      (item) => item.name === "GIT_TOKEN",
    ),
    false,
  );
});

test("importing a .env again never turns a secret plain or loses its overrides", async (t) => {
  const { importChanges } = await import("../src/projectEnvImport.ts");
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    env: [{ name: "API_TOKEN", secret: true }],
  });
  await projects.setSecret(project.id, "API_TOKEN", "invented-base");
  await projects.updateEnv(project.id, {
    set: [{ name: "API_TOKEN", secret: true, hosts: ["ssh:lab"] }],
  });
  await projects.setHostSecret(
    project.id,
    "API_TOKEN",
    "ssh:lab",
    "invented-lab",
  );
  await projects.setHostWithheld(project.id, "ssh:lab", false);
  const stored = (await projects.get(project.id)).env;
  for (const choice of ["replace", "override", "skip-like"]) {
    const set = importChanges(
      stored,
      [{ name: "API_TOKEN", secret: false, choice }],
      "ssh:lab",
    );
    await projects.updateEnv(project.id, { set });
  }
  assert.equal(
    await projects.secretFor(project.id, "API_TOKEN"),
    "invented-base",
  );
  assert.equal(
    await projects.secretForHost(project.id, "API_TOKEN", "ssh:lab"),
    "invented-lab",
  );
});

test("ids that name built-ins are unknown projects, and nothing is polluted", async (t) => {
  const { projects } = await makeStore(t);
  const call = registerHandlers({ projects, connections: {}, claudeMcp: {} });
  for (const id of ["__proto__", "constructor", "toString", 7, null]) {
    assert.equal(await projects.get(id), null);
    for (const attempt of [
      () => projects.updateEnv(id, { set: [{ name: "A", secret: false }] }),
      () => projects.updateMcp(id, { set: {} }),
      () => projects.mergeImport(id, { variables: [{ name: "A" }] }),
      () => projects.setSecret(id, "A", "x"),
      () => projects.setHostSecret(id, "A", "ssh:h", "x"),
      () => projects.setHostWithheld(id, "ssh:h", false),
      () => projects.setHostOverrides(id, "ssh:h", {}),
      () => projects.reviewEnvImport(id, []),
      () => projects.upsert({ id, name: "X" }),
      () => call("projects:git-token:set", id, "x"),
    ])
      await assert.rejects(attempt(), /Unknown project/, `${String(id)}`);
  }
  await projects.delete("__proto__");
  assert.equal({}.env, undefined);
  assert.equal({}.hosts, undefined);
  assert.equal(Object.prototype.mcp, undefined);
});

test("variable and server changes are validated", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({ name: "App" });
  await assert.rejects(projects.updateEnv(project.id, { set: "x" }), /lists/);
  await assert.rejects(projects.updateEnv(project.id, { remove: {} }), /lists/);
  await assert.rejects(
    projects.updateEnv(project.id, {
      set: [{ name: "A", secret: false, availableTo: ["setup", "root"] }],
    }),
    /setup, agent or mcp/,
  );
  await assert.rejects(
    projects.updateMcp(project.id, { remove: "docs" }),
    /Invalid MCP/,
  );
  await assert.rejects(
    projects.updateMcp(project.id, { set: JSON.parse('{"__proto__":{}}') }),
    /Invalid MCP/,
  );
});

test("every writer of the store waits for the others", async (t) => {
  for (let round = 0; round < 5; round++) {
    const { projects } = await makeStore(t);
    const project = await projects.upsert({
      name: "App",
      env: [
        { name: "ONE", secret: true },
        { name: "TWO", secret: true },
      ],
    });
    await Promise.all([
      projects.setHostWithheld(project.id, "ssh:lab", false),
      projects.updateEnv(project.id, {
        set: [{ name: "THREE", secret: false }],
      }),
      projects.setHostOverrides(project.id, "ssh:lab", { PORT: "1" }),
      projects.setSecret(project.id, "ONE", "invented-one"),
      projects.setSecret(project.id, "TWO", "invented-two"),
      projects.setHostSecret(project.id, "ONE", "ssh:lab", "invented-lab-one"),
    ]);
    const after = await projects.get(project.id);
    assert.equal(after.hosts["ssh:lab"].withheld, false);
    assert.deepEqual(after.hosts["ssh:lab"].overrides, { PORT: "1" });
    assert.equal(
      after.env.some((entry) => entry.name === "THREE"),
      true,
    );
    assert.equal(await projects.secretFor(project.id, "ONE"), "invented-one");
    assert.equal(await projects.secretFor(project.id, "TWO"), "invented-two");
    assert.equal(
      await projects.secretForHost(project.id, "ONE", "ssh:lab"),
      "invented-lab-one",
    );
    await Promise.all([
      projects.clearSecret(project.id, "ONE"),
      projects.setSecret(project.id, "TWO", "invented-two-b"),
    ]);
    assert.equal(await projects.secretFor(project.id, "ONE"), null);
    assert.equal(await projects.secretFor(project.id, "TWO"), "invented-two-b");
  }
});

test("a prepare that times out is a timeout in the step it was in, not a clone failure", async (t) => {
  const timedOut = (stderr) =>
    Object.assign(new Error("Connection timed out"), {
      timedOut: true,
      stderr,
    });
  const base = await setup(t);
  const attempt = async (stderr) => {
    const call = registerHandlers({
      projects: base.projects,
      connections: {
        exec: async (_endpoint, command) => {
          if (command.includes("ABSENT")) return "ABSENT\n";
          if (command.includes("FOUND=")) return "";
          throw timedOut(stderr);
        },
      },
      claudeMcp: {},
    });
    return call("projects:host:prepare", base.project.id, base.host.endpoint);
  };
  // Clone finished, install was running, and the output mentions a 403.
  let result = await attempt(
    "SUSHIAI_STEP=start:1\nSUSHIAI_STEP=clone:2\nSUSHIAI_STAGE=install\nnpm ERR! 403 Forbidden\n",
  );
  assert.equal(result.ok, false);
  assert.equal(result.timedOut, true);
  assert.equal(result.stage, "setup");
  assert.equal(result.status, undefined);
  assert.match(result.message, /Timed out after 15 minutes during install/);
  assert.equal(result.steps.find((step) => step.id === "clone").state, "done");
  // No stage marker at all, but the clone had finished: never the clone.
  result = await attempt("SUSHIAI_STEP=start:1\nSUSHIAI_STEP=clone:2\n");
  assert.equal(result.stage, "setup");
  assert.match(result.message, /during install/);
  // A clone that hangs is a clone timeout, still without token advice.
  result = await attempt(
    "SUSHIAI_STEP=start:1\nSUSHIAI_STAGE=clone\nremote: 403 Forbidden\n",
  );
  assert.equal(result.stage, "clone");
  assert.equal(result.status, undefined);
  assert.equal(result.timedOut, true);
});

test("the transport hands back what a timed-out command had printed", async (t) => {
  const host = await makeHost(t);
  await assert.rejects(
    host.connections.exec(host.endpoint, "echo partial-output >&2; sleep 30", {
      timeout: 3000,
    }),
    (error) =>
      error.timedOut === true && error.stderr.includes("partial-output"),
  );
});

test("a checkout whose .git is a file (a worktree) is accepted by the probe and the script alike", async (t) => {
  const { host, project, call } = await setup(t, {
    bin: { npm: '#!/bin/sh\necho ran > "$HOME/npm-saw"\n' },
  });
  const source = project.git.url;
  const folder = path.join(host.home, "sushiai", "my-app");
  await fs.mkdir(path.dirname(folder), { recursive: true });
  execFileSync("git", [
    "-C",
    source,
    "worktree",
    "add",
    "-q",
    "-b",
    "wt",
    folder,
  ]);
  // Worktrees share the source's config, which names itself as its origin.
  execFileSync("git", ["-C", source, "remote", "add", "origin", source]);
  assert.equal((await fs.stat(path.join(folder, ".git"))).isFile(), true);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(await exists(path.join(host.home, "npm-saw")), true);
});

test("a clone that fails never ran a command that could see the install variables", async (t) => {
  const { host, projects, project, call } = await setup(t, {
    bin: { git: '#!/bin/sh\nenv > "$HOME/git-env"\nexit 1\n' },
    env: [{ name: "NPM_TOKEN", secret: true, availableTo: ["setup"] }],
  });
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.equal(result.stage, "clone");
  const seen = await fs.readFile(path.join(host.home, "git-env"), "utf8");
  assert.doesNotMatch(seen, /NPM_TOKEN|invented-npm-token/);
});
