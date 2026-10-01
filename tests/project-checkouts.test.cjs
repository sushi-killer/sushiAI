// Where a project lives on a host: an existing checkout is used where it is,
// a host that is ready starts with no prepare, and the install-variable replay
// leaves nothing in a temp file under any shell.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawnSync } = require("node:child_process");
const {
  makeHost,
  makeStore,
  registerHandlers,
} = require("./helpers/fake-host.cjs");
const { prepareScript } = require("../electron/project-hosts.cjs");

const identity = {
  GIT_AUTHOR_NAME: "Dev",
  GIT_AUTHOR_EMAIL: "dev@example.invalid",
  GIT_COMMITTER_NAME: "Dev",
  GIT_COMMITTER_EMAIL: "dev@example.invalid",
};

/** A bare "remote" every checkout is cloned from. */
async function origin(t) {
  const work = await fs.mkdtemp(path.join(os.tmpdir(), "checkouts-"));
  t.after(() => fs.rm(work, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", work]);
  await fs.writeFile(path.join(work, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", work, "add", "."]);
  execFileSync("git", ["-C", work, "commit", "-qm", "init"], {
    env: { ...process.env, ...identity },
  });
  return work;
}

const NPM =
  '#!/bin/sh\necho x >> "$HOME/npm-runs"\nprintf "%s" "$NPM_TOKEN" > "$HOME/npm-saw"\n';

async function rig(t) {
  const host = await makeHost(t, { bin: { npm: NPM } });
  const { projects } = await makeStore(t);
  const url = await origin(t);
  const project = await projects.upsert({
    name: "App",
    git: { url, defaultBranch: "main" },
    setup: { install: "npm ci", check: "" },
  });
  const commands = [];
  const connections = {
    exec: (endpoint, command, options) => {
      commands.push(command);
      return host.connections.exec(endpoint, command, options);
    },
    inspect: (...args) => host.connections.inspect(...args),
  };
  const call = registerHandlers({ projects, connections, claudeMcp: {} });
  const clone = (dir) => {
    execFileSync("git", ["clone", "-q", url, dir]);
    execFileSync("git", [
      "-C",
      dir,
      "remote",
      "set-url",
      "origin",
      `${url}.git`,
    ]);
  };
  return { host, projects, project, call, commands, clone, url };
}

const exists = (file) =>
  fs.access(file).then(
    () => true,
    () => false,
  );

test("a checkout the owner made elsewhere is used where it is, never cloned a second time", async (t) => {
  const { host, projects, project, call, clone } = await rig(t);
  await fs.mkdir(path.join(host.home, "code"));
  const mine = path.join(host.home, "code", "app");
  clone(mine);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(await fs.realpath(result.path), await fs.realpath(mine));
  assert.equal(result.nonStandard, true);
  assert.match(result.message, /non-standard path/);
  assert.equal(await exists(path.join(host.home, "sushiai", "app")), false);
  // Remembered as the project's folder on that host.
  const folders = await projects.folders(project.id);
  assert.ok(
    folders.some(
      (item) => item.endpoint === host.endpoint && item.cwd === mine,
    ),
  );
});

test("two copies: the project's own folder wins, else the first by path, and it says so", async (t) => {
  const { host, projects, project, call, clone } = await rig(t);
  await fs.mkdir(path.join(host.home, "code"));
  const a = path.join(host.home, "code", "a-app");
  const b = path.join(host.home, "code", "b-app");
  clone(a);
  clone(b);
  let result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(await fs.realpath(result.path), await fs.realpath(a));
  assert.match(result.message, /1 other copy found/);
  // Once the project has a folder there, that one is chosen.
  await projects.attach({
    remote: project.git.url,
    endpoint: host.endpoint,
    cwd: b,
    name: "App",
  });
  const own = await call("projects:host:prepare", project.id, host.endpoint);
  assert.ok(own.ok);
  const folders = (await projects.folders(project.id)).map((f) => f.cwd);
  assert.ok(folders.includes(b));
});

test("with no checkout anywhere the project is cloned into the standard folder", async (t) => {
  const { host, project, call } = await rig(t);
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await exists(path.join(host.home, "sushiai", "app", ".git")),
    true,
  );
  assert.equal(result.nonStandard, undefined);
});

test("a host that is ready starts with no prepare; a changed lock file installs only", async (t) => {
  const { host, project, call, commands } = await rig(t);
  const first = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(first.ok, true, first.message);
  const runs = async () =>
    (await fs.readFile(path.join(host.home, "npm-runs"), "utf8")).length / 2;
  assert.equal(await runs(), 1);
  // The second start asks first: ready, no prepare script, no install.
  commands.length = 0;
  const ready = await call("projects:host:ready", project.id, host.endpoint);
  assert.equal(ready.ready, true);
  assert.equal(
    commands.some((command) => command.includes("SUSHIAI_PREPARED")),
    false,
  );
  assert.equal(await runs(), 1);
  // Asked again at once: remembered, so no ssh call at all.
  commands.length = 0;
  assert.equal(
    (await call("projects:host:ready", project.id, host.endpoint)).ready,
    true,
  );
  assert.deepEqual(commands, []);
  // The lock file changes: not ready for the install only, and a prepare
  // (which forgets what was remembered) installs once and leaves history alone.
  const checkout = path.join(host.home, "sushiai", "app");
  await fs.writeFile(path.join(checkout, "package-lock.json"), '{"n":2}\n');
  const stale = await call(
    "projects:host:prepare",
    project.id,
    host.endpoint,
    false,
    {
      pull: false,
    },
  );
  assert.equal(stale.ok, true, stale.message);
  assert.equal(stale.pull, "skipped:not-asked");
  assert.equal(await runs(), 2);
  const after = await call("projects:host:ready", project.id, host.endpoint);
  assert.equal(after.ready, true);
});

test("a lock file that changed makes the host not ready, with the reason", async (t) => {
  const { host, project, call } = await rig(t);
  await call("projects:host:prepare", project.id, host.endpoint);
  const checkout = path.join(host.home, "sushiai", "app");
  await fs.writeFile(path.join(checkout, "package-lock.json"), '{"n":3}\n');
  // A fresh store entry: the remembered answer is only for a short while, and
  // a prepare forgets it; here nothing was remembered yet for this change.
  const ready = await call("projects:host:ready", project.id, host.endpoint);
  assert.deepEqual([ready.ready, ready.reason], [false, "install-stale"]);
});

test("the install variables never touch a temp file, under zsh, bash or sh", async (t) => {
  const host = await makeHost(t, { bin: { npm: NPM } });
  const url = await origin(t);
  const project = {
    id: "p",
    name: "App",
    slug: "app",
    git: { url, defaultBranch: "main" },
    setup: { install: "npm ci", check: "" },
    env: [],
  };
  const secret = "invented-npm-token-value";
  for (const shell of ["/bin/zsh", "/bin/bash", "/bin/sh"]) {
    if (!(await exists(shell))) continue;
    const tmp = await fs.mkdtemp(path.join(os.tmpdir(), "tmpdir-watch-"));
    t.after(() => fs.rm(tmp, { recursive: true, force: true }));
    const home = await fs.mkdtemp(path.join(host.root, "home-"));
    await fs.writeFile(
      path.join(home, ".gitconfig"),
      "[user]\n\tname = t\n\temail = t@example.invalid\n",
    );
    const { script, input } = prepareScript(project, "", { NPM_TOKEN: secret });
    const ran = spawnSync(shell, ["-c", script], {
      input,
      encoding: "utf8",
      env: {
        PATH: path.join(host.root, "bin"),
        HOME: home,
        TMPDIR: tmp,
      },
    });
    assert.equal(ran.status, 0, `${shell}: ${ran.stderr}`);
    // The install saw the value ...
    assert.equal(await fs.readFile(path.join(home, "npm-saw"), "utf8"), secret);
    // ... and nothing was left in the temp folder, base64 or not.
    assert.deepEqual(await fs.readdir(tmp), [], shell);
  }
});

test("a repository whose default branch is not main is cloned on its own default", async (t) => {
  const host = await makeHost(t, { bin: { npm: NPM } });
  const { projects } = await makeStore(t);
  const work = await fs.mkdtemp(path.join(os.tmpdir(), "trunk-"));
  t.after(() => fs.rm(work, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "trunk", work]);
  await fs.writeFile(path.join(work, "package-lock.json"), "{}\n");
  execFileSync("git", ["-C", work, "add", "."]);
  execFileSync("git", ["-C", work, "commit", "-qm", "init"], {
    env: { ...process.env, ...identity },
  });
  // Made from a folder: no branch is known, so none is demanded.
  const project = await projects.attach({
    remote: work,
    endpoint: "local",
    cwd: "/work/trunk-app",
    name: "Trunk app",
  });
  assert.equal(project.git.defaultBranch, "");
  const call = registerHandlers({
    projects,
    connections: host.connections,
    claudeMcp: {},
  });
  const result = await call("projects:host:prepare", project.id, host.endpoint);
  assert.equal(result.ok, true, result.message);
  assert.equal(
    await exists(path.join(host.home, "sushiai", "trunk-app", ".git")),
    true,
  );
});
