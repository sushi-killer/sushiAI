// Drives the real project IPC handlers over a real store: what an import
// reads from disk stays in the main process, and nothing it brings in is lost
// or widened.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");
const { makeStore, registerHandlers } = require("./helpers/fake-host.cjs");

const REMOTE = "git@example.test:acme/app.git";

async function checkout(t, files) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "import-checkout-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  execFileSync("git", ["init", "-q", "-b", "main", dir]);
  execFileSync("git", ["-C", dir, "remote", "add", "origin", REMOTE]);
  for (const [name, text] of Object.entries(files))
    await fs.writeFile(path.join(dir, name), text);
  const home = path.join(dir, "..", `${path.basename(dir)}-home`);
  await fs.mkdir(home);
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  return { dir, home };
}

const SECRETS = {
  DB_PASSWORD: "invented-db-password",
  DATABASE_URL: "postgres://app:invented-url-pass@db.example.test/app",
  SECRET_KEY_BASE: "invented-key-base",
  MCP_TOKEN: "invented-mcp-literal-token",
};

async function setup(t, files) {
  const { projects } = await makeStore(t);
  const { dir, home } = await checkout(t, files);
  const connections = {
    // The same read the real connections offer, over a local folder.
    inspect: async (_endpoint, { root, path: file }) => {
      try {
        const data = await fs.readFile(path.join(root, file));
        return { base64: data.toString("base64") };
      } catch {
        return null;
      }
    },
  };
  const call = registerHandlers({
    projects,
    connections,
    claudeMcp: new ClaudeMcp({ home }),
  });
  const project = await projects.upsert({
    name: "App",
    git: { url: REMOTE, defaultBranch: "main" },
  });
  return { projects, call, dir, project };
}

const FILES = {
  ".env": [
    `DB_PASSWORD=${SECRETS.DB_PASSWORD}`,
    `DATABASE_URL=${SECRETS.DATABASE_URL}`,
    `SECRET_KEY_BASE=${SECRETS.SECRET_KEY_BASE}`,
    "LOG_LEVEL=debug",
  ].join("\n"),
  ".mcp.json": JSON.stringify({
    mcpServers: {
      github: {
        command: "npx",
        env: { GITHUB_TOKEN: SECRETS.MCP_TOKEN },
      },
    },
  }),
};

test("a store never shows a plain variable's value once it became a secret", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({
    name: "App",
    env: [{ name: "DB_URL", secret: false }],
  });
  await projects.setSecret(project.id, "DB_URL", "postgres://localhost/plain");
  assert.equal(
    (await projects.get(project.id)).env[0].hint,
    "postgres://localhost/plain",
  );
  // The "override" import path turns an existing plain variable into a secret.
  await projects.updateEnv(project.id, {
    set: [{ name: "DB_URL", secret: true }],
  });
  const after = await projects.get(project.id);
  assert.equal(
    JSON.stringify(after).includes("postgres://localhost/plain"),
    false,
  );
  assert.match(after.env[0].hint, /^••••/);
});

test("importing a checkout classifies broadly and returns no values", async (t) => {
  const { call, project, dir, projects } = await setup(t, FILES);
  const result = await call("projects:import-local", project.id, dir);
  const text = JSON.stringify(result);
  for (const value of Object.values(SECRETS))
    assert.equal(text.includes(value), false, `${value} reached the caller`);
  const byName = Object.fromEntries(
    result.project.env.map((entry) => [entry.name, entry]),
  );
  for (const name of ["DB_PASSWORD", "DATABASE_URL", "SECRET_KEY_BASE"])
    assert.equal(byName[name].secret, true, name);
  assert.equal(byName.LOG_LEVEL.secret, false);
  // The values are in the store, for the main process to hand out.
  assert.equal(
    await projects.secretFor(project.id, "DB_PASSWORD"),
    SECRETS.DB_PASSWORD,
  );
  assert.equal(
    await projects.secretFor(project.id, "DATABASE_URL"),
    SECRETS.DATABASE_URL,
  );
  // The literal MCP token became a reference, reachable by MCP only.
  const github = result.project.mcp.mcpServers.github;
  assert.match(github.env.GITHUB_TOKEN, /^\$\{[A-Z_]+\}$/);
  const variable = byName[github.env.GITHUB_TOKEN.slice(2, -1)];
  assert.deepEqual(variable.availableTo, ["mcp"]);
  assert.equal(
    await projects.secretFor(project.id, variable.name),
    SECRETS.MCP_TOKEN,
  );
});

test("a folder that is not this project's checkout is refused", async (t) => {
  const { call, project, dir } = await setup(t, FILES);
  execFileSync("git", [
    "-C",
    dir,
    "remote",
    "set-url",
    "origin",
    "git@example.test:other/repo.git",
  ]);
  await assert.rejects(
    call("projects:import-local", project.id, dir),
    /not a checkout of this project/,
  );
});

test("two imports in a row keep everything the first one brought", async (t) => {
  const { call, project, dir, projects } = await setup(t, FILES);
  await call("projects:import-local", project.id, dir);
  const second = await call(
    "projects:import-mcp-text",
    project.id,
    JSON.stringify({
      mcpServers: {
        docs: {
          url: "https://mcp.example.test",
          headers: { Authorization: "Bearer invented-header-token" },
        },
      },
    }),
  );
  const names = second.project.env.map((entry) => entry.name);
  for (const name of [
    "DB_PASSWORD",
    "DATABASE_URL",
    "SECRET_KEY_BASE",
    "LOG_LEVEL",
  ])
    assert.ok(names.includes(name), `${name} was lost`);
  assert.equal(
    await projects.secretFor(project.id, "DB_PASSWORD"),
    SECRETS.DB_PASSWORD,
  );
  assert.ok(second.project.mcp.mcpServers.github);
  assert.ok(second.project.mcp.mcpServers.docs);
  assert.equal(JSON.stringify(second).includes("invented-header-token"), false);
  // Names that exist are not replaced by a later import.
  const again = await call("projects:import-local", project.id, dir);
  assert.deepEqual(again.addedVariables, []);
  assert.deepEqual(again.addedServers, []);
});

test("imports that run at the same time do not overwrite each other", async (t) => {
  const { projects } = await makeStore(t);
  const project = await projects.upsert({ name: "App" });
  await Promise.all([
    projects.mergeImport(project.id, {
      variables: [{ name: "FIRST", value: "1", secret: false }],
    }),
    projects.mergeImport(project.id, {
      variables: [{ name: "SECOND", value: "2", secret: false }],
    }),
    projects.mergeImport(project.id, {
      servers: { one: { command: "x" } },
    }),
  ]);
  const final = await projects.get(project.id);
  assert.deepEqual(final.env.map((entry) => entry.name).sort(), [
    "FIRST",
    "SECOND",
  ]);
  assert.ok(final.mcp.mcpServers.one);
});

test("a source scanned before its project exists keeps secret values in main", async (t) => {
  const { call, dir, projects } = await setup(t, FILES);
  const scan = await call("projects:scan-source", {
    endpoint: "local",
    root: dir,
    local: true,
  });
  const text = JSON.stringify(scan);
  for (const value of Object.values(SECRETS))
    assert.equal(text.includes(value), false, `${value} reached the caller`);
  const secret = scan.variables.find((item) => item.name === "DB_PASSWORD");
  assert.deepEqual(
    { secret: secret.secret, held: secret.held, value: secret.value },
    { secret: true, held: true, value: undefined },
  );
  assert.equal(
    scan.variables.find((item) => item.name === "LOG_LEVEL").value,
    "debug",
  );
  const created = await call("projects:upsert", {
    name: "New",
    git: { url: REMOTE },
    env: scan.variables.map(({ name, secret: flag, availableTo }) => ({
      name,
      secret: flag,
      availableTo,
    })),
    mcp: { mcpServers: scan.servers },
    importToken: scan.token,
  });
  assert.equal(
    await projects.secretFor(created.id, "DB_PASSWORD"),
    SECRETS.DB_PASSWORD,
  );
  // The token works once.
  const second = await call("projects:upsert", {
    name: "Another",
    git: { url: "git@example.test:acme/other.git" },
    env: [{ name: "DB_PASSWORD", secret: true }],
    importToken: scan.token,
  });
  assert.equal(await projects.secretFor(second.id, "DB_PASSWORD"), null);
});

test("the review of a chosen .env file flags secrets by name and by value", async (t) => {
  const { call, project } = await setup(t, FILES);
  const review = await call(
    "projects:env:review-text",
    project.id,
    [
      "DB_PASSWORD=x",
      "TOKEN=y",
      "DATABASE_URL=postgres://u:p@h/db",
      "PORT=3000",
    ].join("\n"),
  );
  assert.deepEqual(
    review.map((entry) => [entry.name, entry.secret]),
    [
      ["DB_PASSWORD", true],
      ["TOKEN", true],
      ["DATABASE_URL", true],
      ["PORT", false],
    ],
  );
});

test("a local install runs once per lock file, with setup variables, and leaves a plain folder alone", async (t) => {
  const { call, project, dir, projects } = await setup(t, {
    "package-lock.json": "{}\n",
  });
  const log = path.join(dir, "..", `${path.basename(dir)}-installs`);
  t.after(() => fs.rm(log, { force: true }));
  await projects.upsert({
    ...(await projects.get(project.id)),
    setup: { install: 'echo "$SETUP_ONLY" >> "$INSTALL_LOG"', check: "" },
  });
  await projects.updateEnv(project.id, {
    set: [
      { name: "INSTALL_LOG", secret: false, availableTo: ["setup"] },
      { name: "SETUP_ONLY", secret: true, availableTo: ["setup"] },
    ],
  });
  await projects.setSecret(project.id, "INSTALL_LOG", log);
  await projects.setSecret(project.id, "SETUP_ONLY", "invented-setup-value");
  const first = await call("projects:local-install", project.id, dir);
  const second = await call("projects:local-install", project.id, dir);
  assert.equal(first.ran, true);
  assert.equal(second.ran, true); // the script ran; it skipped the command
  assert.equal((await fs.readFile(log, "utf8")).trim(), "invented-setup-value");
  // A folder that is not a git checkout is not installed into.
  const plain = await fs.mkdtemp(path.join(os.tmpdir(), "plain-"));
  t.after(() => fs.rm(plain, { recursive: true, force: true }));
  assert.deepEqual(await call("projects:local-install", project.id, plain), {
    ran: false,
    seconds: 0,
  });
  assert.deepEqual(await fs.readdir(plain), []);
});

test("a source's held values go only into a project that call creates, and only into secrets", async (t) => {
  const { call, dir, projects, project } = await setup(t, FILES);
  const scan = async () =>
    call("projects:scan-source", { endpoint: "local", root: dir, local: true });
  // Aimed at a project that already exists: nothing is written.
  await projects.updateEnv(project.id, {
    set: [{ name: "DB_PASSWORD", secret: true }],
  });
  let held = await scan();
  await call("projects:upsert", {
    ...project,
    env: [{ name: "DB_PASSWORD", secret: true }],
    importToken: held.token,
  });
  assert.equal(await projects.secretFor(project.id, "DB_PASSWORD"), null);
  // A new project that declares the entry plain: the held secret is not
  // written into it.
  held = await scan();
  const plain = await call("projects:upsert", {
    name: "Plain",
    git: { url: "git@example.test:acme/plain.git" },
    env: [{ name: "DB_PASSWORD", secret: false }],
    importToken: held.token,
  });
  assert.equal(await projects.secretFor(plain.id, "DB_PASSWORD"), null);
});
