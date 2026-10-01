const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  readiness,
  hostProbeScript,
  prepareScript,
  prepareSteps,
} = require("../electron/project-hosts.cjs");
const { Projects } = require("../electron/projects.cjs");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { Connections } = require("../electron/connections.cjs");

test("fake ssh probe reports checkout, setup, CLI, MCP, and secret readiness", () => {
  assert.match(
    hostProbeScript(undefined, "Demo Project"),
    /sushiai\/demo-project/,
  );
  const matrix = readiness({
    now: 10,
    cwd: "/home/dev/sushiai/demo",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo.git" },
      setup: { install: "npm ci", check: "npm test" },
      mcp: { mcpServers: { local: { url: "https://mcp.example.test" } } },
      env: [{ name: "TOKEN", secret: true, hasValue: true }],
      hosts: { "ssh:user@devbox": { trusted: true } },
    },
    output:
      "home=/home/dev\nroot=/home/dev/sushiai/demo\nstandard=1\nremote=https://example.invalid/team/demo.git\ngit=1\nclaude=1\nclaude_login=1\ncodex=1\n",
  });
  assert.equal(matrix.checkout.ok, true);
  assert.equal(matrix.checkout.nonStandard, false);
  assert.equal(matrix.setup.ok, true);
  assert.equal(matrix.clis.claude.loggedIn, true);
  assert.equal(matrix.clis.codex.loggedIn, false);
  assert.equal(matrix.clis.git, true);
  assert.equal(matrix.clis.cc, false);
  assert.deepEqual(matrix.mcp, { ok: true, count: 1, missing: [] });
  assert.deepEqual(matrix.secrets, { ok: true, count: 1 });
  assert.equal(matrix.trusted, true);
});

test("the hosts readiness IPC probes through a fake ssh executable", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "fake-ssh-hosts-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const ssh = path.join(dir, "ssh");
  await fs.writeFile(
    ssh,
    '#!/bin/sh\nprintf "home=/home/user\\nroot=/home/user/sushiai/demo\\nstandard=1\\nremote=https://example.invalid/team/demo\\ngit=1\\nclaude=1\\nclaude_login=1\\n"\n',
    { mode: 0o755 },
  );
  const connections = new Connections(dir, { ssh });
  const profile = await connections.save({
    id: "00000000-0000-4000-8000-000000000001",
    name: "Devbox",
    host: "user@devbox",
    socket: "~/.sushiai/orchestrator/orchd.sock",
  });
  const output = await connections.exec(`ssh:${profile.id}`, hostProbeScript());
  const matrix = readiness({
    output,
    project: {
      host: `ssh:${profile.id}`,
      git: { url: "git@example.invalid:team/demo.git" },
      setup: {},
      mcp: {},
      env: [],
    },
  });
  assert.equal(matrix.checkout.ok, true);
  assert.equal(matrix.checkout.nonStandard, false);
  assert.equal(matrix.clis.claude.loggedIn, true);
  await connections.disconnect(`ssh:${profile.id}`);
});

test("checkout mismatch is flagged and a non-standard location is detected", () => {
  const matrix = readiness({
    cwd: "/srv/checkout/demo",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo" },
      setup: {},
      mcp: {},
      env: [],
    },
    output:
      "home=/home/dev\nroot=/srv/checkout/demo\nremote=https://example.invalid/other/repo\n",
  });
  assert.equal(matrix.checkout.ok, false);
  assert.equal(matrix.checkout.nonStandard, true);
});

test("missing checkout, setup, CLIs, MCP, secrets, and trust report missing states", () => {
  const matrix = readiness({
    output: "home=/home/dev\nroot=/home/dev/sushiai/demo\nstandard=1\n",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo" },
      setup: {},
      mcp: {},
      env: [],
      hosts: {},
    },
  });
  assert.equal(matrix.checkout.ok, false);
  assert.equal(matrix.setup.ok, false);
  assert.equal(matrix.clis.claude.installed, false);
  assert.equal(matrix.clis.codex.installed, false);
  assert.equal(matrix.mcp.ok, false);
  assert.equal(matrix.secrets.ok, false);
  assert.equal(matrix.trusted, false);
});

test("revoking project host trust blocks stored secrets on the next read", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-hosts-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const safeStorage = {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => Buffer.from(value),
    decryptString: (value) => value.toString(),
  };
  const projects = new Projects({ userDataDir: dir, safeStorage });
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "TOKEN", secret: true }],
  });
  await projects.setSecret(project.id, "TOKEN", "invented-secret-value");
  await projects.upsert({
    ...project,
    hosts: { "ssh:user@devbox": { trusted: true } },
  });
  assert.equal(
    (await projects.get(project.id)).hosts?.["ssh:user@devbox"]?.trusted,
    undefined,
  );
  await projects.setHostTrust(project.id, "ssh:user@devbox", true);
  await projects.setHostOverrides(project.id, "ssh:user@devbox", {
    backend: "local",
  });
  assert.equal(
    await projects.secretForHost(project.id, "TOKEN", "ssh:user@devbox"),
    "invented-secret-value",
  );
  assert.deepEqual(
    (await projects.get(project.id)).hosts["ssh:user@devbox"].overrides,
    { backend: "local" },
  );
  await projects.setHostTrust(project.id, "ssh:user@devbox", false);
  assert.equal(
    await projects.secretForHost(project.id, "TOKEN", "ssh:user@devbox"),
    null,
  );
});

test("prepare reports each step it got through, and the one that failed", async (t) => {
  const { execFileSync, spawnSync } = require("node:child_process");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "prepare-steps-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const origin = path.join(dir, "origin");
  const git = (cwd, ...args) =>
    execFileSync("git", args, {
      cwd,
      stdio: "pipe",
      env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" },
    });
  await fs.mkdir(origin);
  git(origin, "init", "-q", "-b", "main");
  await fs.writeFile(path.join(origin, "a.txt"), "a");
  git(origin, "add", ".");
  git(
    origin,
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@example.test",
    "commit",
    "-q",
    "-m",
    "init",
  );
  const project = {
    name: "Steps Demo",
    git: { url: origin, defaultBranch: "main" },
    setup: { install: "true", check: "true" },
  };
  const run = (p, home) =>
    spawnSync("/bin/sh", ["-c", prepareScript(p, "").script], {
      env: { ...process.env, HOME: home, GIT_CONFIG_GLOBAL: "/dev/null" },
      encoding: "utf8",
    });
  const home = path.join(dir, "home");
  await fs.mkdir(home);
  const ok = run(project, home);
  assert.equal(ok.status, 0, ok.stderr);
  const steps = prepareSteps(project, ok.stdout);
  assert.deepEqual(
    steps.map((step) => [step.id, step.state]),
    [
      ["clone", "done"],
      ["install", "done"],
      ["check", "done"],
    ],
  );
  assert.ok(steps.every((step) => typeof step.seconds === "number"));

  // A clone that fails leaves install and check pending.
  const broken = { ...project, git: { url: path.join(dir, "missing") } };
  const failed = run(broken, path.join(dir, "home2"));
  assert.notEqual(failed.status, 0);
  assert.deepEqual(
    prepareSteps(broken, failed.stdout, "clone").map((step) => [
      step.id,
      step.state,
    ]),
    [
      ["clone", "failed"],
      ["install", "pending"],
      ["check", "pending"],
    ],
  );
});

test("an MCP server whose command the host lacks is reported as missing", () => {
  const project = {
    git: { url: "https://example.invalid/team/demo.git" },
    setup: {},
    env: [],
    mcp: {
      mcpServers: {
        files: { command: "npx", args: ["server-files"] },
        docs: { command: "uvx", args: ["server-docs"] },
        off: { command: "bunx" },
        web: { url: "https://mcp.example.test/web" },
      },
      disabledMcpServers: ["off"],
    },
  };
  const script = hostProbeScript(undefined, "Demo", project);
  assert.match(script, /command -v npx/);
  assert.match(script, /command -v uvx/);
  assert.doesNotMatch(script, /bunx/);
  const matrix = readiness({
    project,
    output: "home=/h\nmcpcmd=npx:0\nmcpcmd=uvx:1\n",
  });
  assert.deepEqual(matrix.mcp, {
    ok: false,
    count: 4,
    missing: [{ name: "files", command: "npx" }],
  });
});

test("running the install, then the probe, says the next run will not reinstall until the lock file changes", async (t) => {
  const { spawnSync, execFileSync } = require("node:child_process");
  const { installScript } = require("../electron/project-hosts.cjs");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "probe-lock-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const home = path.join(dir, "home");
  const root = path.join(home, "sushiai", "demo");
  await fs.mkdir(root, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", root]);
  await fs.writeFile(path.join(root, "package-lock.json"), "{}\n");
  const project = {
    git: { url: "https://example.invalid/team/demo.git" },
    setup: { install: 'echo ran >> "$HOME/installs"' },
    env: [],
    mcp: {},
  };
  const sh = (script) =>
    spawnSync("/bin/sh", ["-c", script], {
      cwd: root,
      env: { ...process.env, HOME: home },
      encoding: "utf8",
    });
  const probe = () =>
    readiness({
      output: sh(hostProbeScript(root, "Demo")).stdout,
      project,
      cwd: root,
    }).setup;
  const installs = async () =>
    (await fs.readFile(path.join(home, "installs"), "utf8").catch(() => ""))
      .split("\n")
      .filter(Boolean).length;

  // Never installed from: the next run installs.
  assert.equal(probe().stale, true);
  assert.equal(probe().lockFile, "package-lock.json");
  assert.equal(sh(installScript(project)).status, 0);
  assert.equal(await installs(), 1);
  assert.equal(probe().stale, false);
  // The same lock file: the install does not run again.
  sh(installScript(project));
  assert.equal(await installs(), 1);
  // A changed lock file: stale again, and the next install runs.
  await fs.writeFile(path.join(root, "package-lock.json"), '{"v":2}\n');
  assert.equal(probe().stale, true);
  sh(installScript(project));
  assert.equal(await installs(), 2);
  // The checkout's working tree is never written to.
  assert.equal(
    execFileSync("git", ["-C", root, "status", "--porcelain"], {
      encoding: "utf8",
    }).includes("sushiai"),
    false,
  );
});

test("the lock marker works in a worktree, and a folder that is not a checkout uses the app's own cache", async (t) => {
  const { spawnSync, execFileSync } = require("node:child_process");
  const { installScript } = require("../electron/project-hosts.cjs");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "probe-lock-wt-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const home = path.join(dir, "home");
  await fs.mkdir(home);
  const main = path.join(dir, "main");
  const gitEnv = {
    ...process.env,
    GIT_AUTHOR_NAME: "Dev",
    GIT_AUTHOR_EMAIL: "dev@example.invalid",
    GIT_COMMITTER_NAME: "Dev",
    GIT_COMMITTER_EMAIL: "dev@example.invalid",
  };
  execFileSync("git", ["init", "-q", "-b", "main", main]);
  await fs.writeFile(path.join(main, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", main, "add", "."]);
  execFileSync("git", ["-C", main, "commit", "-qm", "init"], { env: gitEnv });
  const worktree = path.join(dir, "wt");
  execFileSync("git", [
    "-C",
    main,
    "worktree",
    "add",
    "-q",
    "-b",
    "feature",
    worktree,
  ]);
  const project = { setup: { install: 'echo ran >> "$HOME/installs"' } };
  const run = (cwd) =>
    spawnSync("/bin/sh", ["-c", installScript(project)], {
      cwd,
      env: { ...process.env, HOME: home },
      encoding: "utf8",
    });
  assert.equal(run(worktree).status, 0);
  run(worktree);
  assert.equal(
    (await fs.readFile(path.join(home, "installs"), "utf8"))
      .split("\n")
      .filter(Boolean).length,
    1,
  );
  assert.equal(
    execFileSync("git", ["-C", worktree, "status", "--porcelain"], {
      encoding: "utf8",
    }),
    "",
  );
  // No git here: the marker goes to a cache of the app's own.
  const plain = path.join(dir, "plain");
  await fs.mkdir(plain);
  await fs.writeFile(path.join(plain, "package-lock.json"), "{}\n");
  assert.equal(run(plain).status, 0);
  assert.deepEqual(await fs.readdir(plain), ["package-lock.json"]);
  assert.equal(
    (await fs.readdir(path.join(home, ".sushiai", "lock-hashes"))).length,
    1,
  );
});

test("a marker that cannot be written does not turn a good install into a failure", async (t) => {
  const { spawnSync, execFileSync } = require("node:child_process");
  const { installScript } = require("../electron/project-hosts.cjs");
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "probe-lock-ro-"));
  t.after(async () => {
    await fs.chmod(path.join(dir, "repo", ".git"), 0o755).catch(() => {});
    await fs.rm(dir, { recursive: true, force: true });
  });
  const repo = path.join(dir, "repo");
  execFileSync("git", ["init", "-q", "-b", "main", repo]);
  await fs.writeFile(path.join(repo, "package-lock.json"), "{}\n");
  await fs.chmod(path.join(repo, ".git"), 0o555);
  const run = spawnSync(
    "/bin/sh",
    ["-c", `set -e\n${installScript({ setup: { install: "true" } })}`],
    { cwd: repo, env: { ...process.env, HOME: dir }, encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
});
