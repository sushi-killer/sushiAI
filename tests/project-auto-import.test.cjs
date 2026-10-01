const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  mergeEnvSources,
  secretizeMcpServers,
} = require("../electron/project-import.cjs");
const { localCreate, pullMessage } = require("../electron/project-git.cjs");
const { prepareScript } = require("../electron/project-hosts.cjs");
const { ClaudeMcp } = require("../electron/claude-mcp.cjs");

const identity = {
  GIT_AUTHOR_NAME: "Dev",
  GIT_AUTHOR_EMAIL: "dev@example.invalid",
  GIT_COMMITTER_NAME: "Dev",
  GIT_COMMITTER_EMAIL: "dev@example.invalid",
};
const git = (cwd, ...args) =>
  execFileSync("git", ["-C", cwd, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...identity },
  }).trim();

async function upstream(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-pull-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const origin = path.join(dir, "origin");
  execFileSync("git", ["init", "-b", "main", origin], { env: process.env });
  await fs.writeFile(path.join(origin, "a.txt"), "one\n");
  git(origin, "add", ".");
  git(origin, "commit", "-m", "first");
  const publish = async (file, text) => {
    await fs.writeFile(path.join(origin, file), text);
    git(origin, "add", ".");
    git(origin, "commit", "-m", `change ${file}`);
  };
  return { dir, origin, publish };
}

test(".env.local beats .env beats .env.example, and example secrets stay empty", () => {
  const merged = mergeEnvSources({
    example: "API_TOKEN=placeholder\nMODE=example\nONLY_EXAMPLE=1\n",
    env: "API_TOKEN=invented-real-token\nMODE=env\n",
    local: "MODE=local\nEXTRA_KEY=invented-key\n",
  });
  const byName = Object.fromEntries(merged.map((entry) => [entry.name, entry]));
  assert.equal(byName.API_TOKEN.value, "invented-real-token");
  assert.equal(byName.API_TOKEN.secret, true);
  assert.equal(byName.MODE.value, "local");
  assert.equal(byName.MODE.secret, false);
  assert.equal(byName.EXTRA_KEY.value, "invented-key");
  assert.equal(byName.ONLY_EXAMPLE.value, "1");
  const exampleOnly = mergeEnvSources({ example: "SOME_SECRET=change-me\n" });
  assert.equal(exampleOnly[0].value, "");
});

test("MCP env and header secrets become ${VAR} references with matching secrets", () => {
  const { servers, secrets } = secretizeMcpServers({
    github: {
      command: "npx",
      args: ["server"],
      env: { GITHUB_TOKEN: "invented-gh-value", LOG_LEVEL: "debug" },
    },
    remote: {
      url: "https://mcp.example.invalid/sse",
      headers: {
        Authorization: "Bearer invented-bearer-value",
        "X-Trace": "on",
        "X-Api-Key": "${ALREADY_SET}",
      },
    },
  });
  assert.equal(servers.github.env.GITHUB_TOKEN, "${GITHUB_TOKEN}");
  assert.equal(servers.github.env.LOG_LEVEL, "debug");
  assert.equal(servers.remote.headers.Authorization, "Bearer ${AUTHORIZATION}");
  assert.equal(servers.remote.headers["X-Trace"], "on");
  assert.equal(servers.remote.headers["X-Api-Key"], "${ALREADY_SET}");
  assert.deepEqual(secrets, {
    GITHUB_TOKEN: "invented-gh-value",
    AUTHORIZATION: "invented-bearer-value",
  });
  assert.doesNotMatch(JSON.stringify(servers), /invented/);
});

test("a clashing secret name gets a server-specific variable", () => {
  const { servers, secrets } = secretizeMcpServers(
    {
      a: { command: "x", env: { API_KEY: "invented-one" } },
      b: { command: "y", env: { API_KEY: "invented-two" } },
    },
    {},
  );
  assert.equal(servers.a.env.API_KEY, "${API_KEY}");
  assert.equal(servers.b.env.API_KEY, "${B_API_KEY}");
  assert.deepEqual(secrets, {
    API_KEY: "invented-one",
    B_API_KEY: "invented-two",
  });
});

test("local create fast-forwards an existing checkout and skips unsafe pulls", async (t) => {
  const { dir, origin, publish } = await upstream(t);
  const checkout = path.join(dir, "checkout");
  const first = await localCreate({ url: origin, cwd: checkout });
  assert.equal(first.pull, "cloned");
  assert.equal(
    (await localCreate({ url: origin, cwd: checkout })).pull,
    "current",
  );

  await publish("b.txt", "two\n");
  const updated = await localCreate({ url: origin, cwd: checkout });
  assert.equal(updated.pull, "updated");
  assert.equal(updated.message, "Updated");
  assert.equal(
    await fs.readFile(path.join(checkout, "b.txt"), "utf8"),
    "two\n",
  );

  await publish("c.txt", "three\n");
  await fs.writeFile(path.join(checkout, "a.txt"), "local edit\n");
  const dirty = await localCreate({ url: origin, cwd: checkout });
  assert.equal(dirty.pull, "skipped:local-changes");
  assert.equal(dirty.message, pullMessage("skipped:local-changes"));
  assert.equal(
    await fs.readFile(path.join(checkout, "a.txt"), "utf8"),
    "local edit\n",
  );
  await assert.rejects(fs.access(path.join(checkout, "c.txt")));

  git(checkout, "checkout", "--", "a.txt");
  await fs.writeFile(path.join(checkout, "own.txt"), "mine\n");
  git(checkout, "add", ".");
  git(checkout, "commit", "-m", "diverge");
  await publish("d.txt", "four\n");
  const diverged = await localCreate({ url: origin, cwd: checkout });
  assert.equal(diverged.pull, "skipped:not-fast-forward");
  assert.equal(
    await fs.readFile(path.join(checkout, "own.txt"), "utf8"),
    "mine\n",
  );

  // A folder that belongs to another repository is refused, not reused.
  await assert.rejects(
    localCreate({
      url: "https://example.invalid/other/repo.git",
      cwd: checkout,
    }),
    /different repository/,
  );
});

test("prepare script pulls an existing remote checkout and reports the state", async (t) => {
  const { dir, origin, publish } = await upstream(t);
  const home = path.join(dir, "home");
  await fs.mkdir(home);
  const project = {
    name: "Demo Project",
    git: { url: origin, defaultBranch: "main" },
    setup: {},
  };
  const run = () =>
    execFileSync("sh", ["-c", prepareScript(project, "").script], {
      encoding: "utf8",
      env: { ...process.env, ...identity, HOME: home },
    });
  const pullOf = (output) => output.match(/^SUSHIAI_PULL=(.+)$/m)?.[1];
  const target = path.join(home, "sushiai", "demo-project");

  assert.equal(pullOf(run()), "cloned");
  assert.equal(pullOf(run()), "current");
  await publish("b.txt", "two\n");
  assert.equal(pullOf(run()), "updated");
  assert.equal(await fs.readFile(path.join(target, "b.txt"), "utf8"), "two\n");

  await publish("c.txt", "three\n");
  await fs.writeFile(path.join(target, "a.txt"), "local edit\n");
  const skipped = run();
  assert.equal(pullOf(skipped), "skipped:local-changes");
  assert.match(skipped, /SUSHIAI_PREPARED=/);
  assert.equal(
    await fs.readFile(path.join(target, "a.txt"), "utf8"),
    "local edit\n",
  );
  await assert.rejects(fs.access(path.join(target, "c.txt")));
});

test("importable MCP servers merge .mcp.json with the project entry, not user scope", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "mcp-importable-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const home = path.join(dir, "home");
  const root = path.join(dir, "repo");
  await fs.mkdir(home);
  await fs.mkdir(root);
  await fs.writeFile(
    path.join(root, ".mcp.json"),
    JSON.stringify({
      mcpServers: { repo: { command: "a" }, both: { command: "from-file" } },
    }),
  );
  await fs.writeFile(
    path.join(home, ".claude.json"),
    JSON.stringify({
      mcpServers: { personal: { command: "p" } },
      projects: {
        [path.resolve(root)]: {
          mcpServers: { both: { command: "from-claude-json" } },
        },
      },
    }),
  );
  const { servers } = await new ClaudeMcp({ home }).importable(root);
  assert.deepEqual(Object.keys(servers).sort(), ["both", "repo"]);
  assert.equal(servers.both.command, "from-claude-json");
});
