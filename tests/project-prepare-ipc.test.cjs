// Prepares a project on a fake SSH host through the real IPC handler, the
// real script and a real git: what a failure is called, what it blames, and
// what is (and is not) sent along.
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

const identity = {
  GIT_AUTHOR_NAME: "Dev",
  GIT_AUTHOR_EMAIL: "dev@example.invalid",
  GIT_COMMITTER_NAME: "Dev",
  GIT_COMMITTER_EMAIL: "dev@example.invalid",
};

async function origin(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "prepare-origin-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  await fs.writeFile(path.join(dir, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", dir, "add", "."]);
  execFileSync("git", ["-C", dir, "commit", "-qm", "init"], {
    env: { ...process.env, ...identity },
  });
  return dir;
}

async function setup(t, { bin, setupSteps = {}, env = [] } = {}) {
  const host = await makeHost(t, { bin });
  const { projects } = await makeStore(t);
  const url = await origin(t);
  const project = await projects.upsert({
    name: "My App!",
    git: { url, defaultBranch: "main" },
    setup: { install: "npm ci", check: "npm test", ...setupSteps },
    env,
  });
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  return { host, projects, project, call, url };
}

const NPM_OK = '#!/bin/sh\necho "$NPM_TOKEN" > "$HOME/npm-saw"\nexit 0\n';

test("a clean prepare reports every step as done, under the folder name of 'My App!'", async (t) => {
  const { host, project, call } = await setup(t, { bin: { npm: NPM_OK } });
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(result.path.endsWith("/sushiai/my-app"), true);
  assert.deepEqual(
    result.steps.map((step) => [step.id, step.state]),
    [
      ["clone", "done"],
      ["install", "done"],
      ["check", "done"],
    ],
  );
});

test("an install that fails with a 403 is an install failure, not a git token problem", async (t) => {
  const { host, project, call } = await setup(t, {
    bin: {
      npm: '#!/bin/sh\necho "npm ERR! 403 Forbidden - GET https://registry.example.test/pkg" >&2\nexit 1\n',
    },
  });
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.equal(result.stage, "setup");
  assert.equal(result.status, undefined);
  assert.deepEqual(
    result.steps.map((step) => [step.id, step.state]),
    [
      ["clone", "done"],
      ["install", "failed"],
      ["check", "pending"],
    ],
  );
  assert.match(result.message, /npm ERR! 403/);
});

test("a clone refused with a 403 is the git token's problem and stops there", async (t) => {
  const { host, project, call } = await setup(t, {
    bin: {
      npm: NPM_OK,
      git: `#!/bin/sh\ncase "$*" in *clone*) echo "fatal: unable to access: The requested URL returned error: 403" >&2; exit 128;; esac\nexec REAL_GIT "$@"\n`,
    },
  });
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.equal(result.stage, "clone");
  assert.equal(result.status, 403);
  assert.deepEqual(
    result.steps.map((step) => [step.id, step.state]),
    [
      ["clone", "failed"],
      ["install", "pending"],
      ["check", "pending"],
    ],
  );
});

test("a folder of another repository is refused before anything runs in it", async (t) => {
  const { host, project, call } = await setup(t, { bin: { npm: NPM_OK } });
  const folder = path.join(host.home, "sushiai", "my-app");
  await fs.mkdir(folder, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", folder]);
  execFileSync("git", [
    "-C",
    folder,
    "remote",
    "add",
    "origin",
    "git@example.test:other/repo.git",
  ]);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, false);
  assert.equal(result.stage, "clone");
  assert.match(result.message, /different repository/);
  await assert.rejects(fs.access(path.join(host.home, "npm-saw")));
});

test("the same repository over ssh and https is not a mismatch", async (t) => {
  const { host, project, projects, call } = await setup(t, {
    bin: { npm: NPM_OK },
  });
  await projects.upsert({
    ...(await projects.get(project.id)),
    git: { url: "https://Example.test/Acme/App", defaultBranch: "main" },
  });
  const folder = path.join(host.home, "sushiai", "my-app");
  await fs.mkdir(folder, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", folder]);
  execFileSync("git", [
    "-C",
    folder,
    "remote",
    "add",
    "origin",
    "git@example.test:acme/app.git",
  ]);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
});

test("setup variables reach the install over stdin, never argv, and the git token is not among them", async (t) => {
  const { host, projects, project, call } = await setup(t, {
    bin: { npm: NPM_OK },
    env: [
      { name: "NPM_TOKEN", secret: true, availableTo: ["setup"] },
      { name: "GIT_TOKEN", secret: true, availableTo: ["setup", "agent"] },
      { name: "AGENT_ONLY", secret: true, availableTo: ["agent"] },
    ],
  });
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  await projects.setSecret(project.id, "GIT_TOKEN", "invented-git-token");
  await projects.setSecret(project.id, "AGENT_ONLY", "invented-agent-value");
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    (await fs.readFile(path.join(host.home, "npm-saw"), "utf8")).trim(),
    "invented-npm-token",
  );
  const argv = await fs.readFile(host.log, "utf8");
  for (const value of [
    "invented-npm-token",
    "invented-git-token",
    "invented-agent-value",
  ])
    assert.equal(argv.includes(value), false, `${value} was on a command line`);
  // Nothing was left on disk on the host.
  const left = await fs
    .readdir(path.join(host.home, ".sushiai"))
    .catch(() => []);
  assert.deepEqual(left, []);
});

test("a host switched off for the project is prepared with no value at all", async (t) => {
  const { host, projects, project, call } = await setup(t, {
    bin: {
      npm: '#!/bin/sh\nprintf "[%s]" "$NPM_TOKEN" > "$HOME/npm-saw"\n',
    },
    env: [{ name: "NPM_TOKEN", secret: true, availableTo: ["setup"] }],
  });
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  await projects.setHostWithheld(project.id, host.endpoint, true);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await fs.readFile(path.join(host.home, "npm-saw"), "utf8"),
    "[]",
  );
});

test("a host that was just added gets the values with no question asked", async (t) => {
  const { host, projects, project, call } = await setup(t, {
    bin: {
      npm: '#!/bin/sh\nprintf "[%s]" "$NPM_TOKEN" > "$HOME/npm-saw"\n',
    },
    env: [{ name: "NPM_TOKEN", secret: true, availableTo: ["setup"] }],
  });
  await projects.setSecret(project.id, "NPM_TOKEN", "invented-npm-token");
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await fs.readFile(path.join(host.home, "npm-saw"), "utf8"),
    "[invented-npm-token]",
  );
});
