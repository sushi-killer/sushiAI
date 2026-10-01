const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execute = promisify(execFile);

const {
  suggestWorktreeBranch,
  worktreeBranchError,
  worktreeCreateParams,
  herdrWorkspaceKey,
  launchesInWorktree,
} = require("../src/workspace/worktree.ts");
const {
  worktreeBranchError: worktreeBranchErrorMain,
  worktreePath,
  worktreeAddArgs,
  createWorktree,
} = require("../electron/worktree.cjs");
const {
  registerHerdrExtension,
} = require("../electron/extensions/builtin-herdr.cjs");
const { registerProjectIpc } = require("../electron/ipc/projects.cjs");
const { Connections } = require("../electron/connections.cjs");

const BRANCH_CASES = [
  ["feature/x", true],
  ["sushi/20240101-0930", true],
  ["-bad", false],
  ["a..b", false],
  ["a b", false],
  ["x.lock", false],
  ["a//b", false],
  ["@{", false],
  ["", false],
  ["a".repeat(101), false],
  ["a".repeat(100), true],
  ["trailing/", false],
  ["trailing.", false],
  ["has~tilde", false],
  ["has^caret", false],
  ["has:colon", false],
  ["has?question", false],
  ["has*star", false],
  ["has[bracket", false],
  ["has\\backslash", false],
  ["has\ttab", false],
  ["/leading-slash", false],
];

test("worktreeBranchError accepts a valid/invalid branch table (renderer copy)", () => {
  for (const [name, valid] of BRANCH_CASES) {
    const error = worktreeBranchError(name);
    assert.equal(
      error === "",
      valid,
      `${JSON.stringify(name)} expected valid=${valid}, got error ${JSON.stringify(error)}`,
    );
  }
});

test("worktreeBranchError agrees between the renderer copy and the main-process copy", () => {
  for (const [name, valid] of BRANCH_CASES) {
    const rendererValid = worktreeBranchError(name) === "";
    const mainValid = worktreeBranchErrorMain(name) === "";
    assert.equal(rendererValid, valid);
    assert.equal(mainValid, valid);
  }
});

test("suggestWorktreeBranch formats a fixed local date as sushi/YYYYMMDD-HHmm", () => {
  assert.equal(
    suggestWorktreeBranch(new Date(2024, 0, 5, 9, 7, 0)),
    "sushi/20240105-0907",
  );
  assert.equal(
    suggestWorktreeBranch(new Date(2026, 11, 31, 23, 59, 0)),
    "sushi/20261231-2359",
  );
});

test("worktreeCreateParams builds the Herdr worktree.create payload", () => {
  assert.deepEqual(
    worktreeCreateParams({ cwd: "/Users/dev/app" }, "feature/x"),
    {
      cwd: "/Users/dev/app",
      branch: "feature/x",
      base: "refs/heads/main",
      label: "feature/x",
      focus: false,
    },
  );
  assert.equal(
    worktreeCreateParams(
      { cwd: "/Users/dev/app" },
      "feature/x",
      "release/stable",
    ).base,
    "release/stable",
  );
});

test("herdrWorkspaceKey builds the sushiAI id for local and SSH endpoints", () => {
  assert.equal(
    herdrWorkspaceKey("unix:///tmp/herdr.sock", "ws-1"),
    "herdr:v2:unix%3A%2F%2F%2Ftmp%2Fherdr.sock:ws-1",
  );
  assert.equal(
    herdrWorkspaceKey("ssh:devbox", "ws-2"),
    "herdr:v2:ssh%3Adevbox:ws-2",
  );
  assert.notEqual(
    herdrWorkspaceKey("/tmp/first.sock", "same"),
    herdrWorkspaceKey("/tmp/second.sock", "same"),
  );
  assert.notEqual(herdrWorkspaceKey("a:b", "c"), herdrWorkspaceKey("a", "b:c"));
});

test("the Herdr allowlist accepts worktree.create without touching any other method", async () => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-"),
  );
  const socketPath = path.join(directory, "no-gateway-here.sock");
  const id = (value) => value;
  const handlers = new Map();
  registerHerdrExtension({
    handle: (channel, callback) => handlers.set(channel, callback),
    getConnections: () => ({
      socket: async () => socketPath,
      inspect: async () => ({ cwd: "/tmp", base: "test-commit" }),
    }),
    id,
  });
  const herdr = handlers.get("herdr");
  // An unknown method is refused before ever touching the socket.
  await assert.rejects(
    herdr("local", "worktree.destroy", { branch: "x" }),
    /Invalid Herdr request/,
  );
  // worktree.create passes the allowlist check and reaches the (absent)
  // socket instead - a connection failure, not "Invalid Herdr request".
  await assert.rejects(
    herdr("local", "worktree.create", { cwd: "/tmp" }),
    (error) => {
      assert.doesNotMatch(error.message, /Invalid Herdr request/);
      return true;
    },
  );
  await fs.rm(directory, { recursive: true, force: true });
});

test("worktreePath places a new worktree beside the repo root, slugging slashes", () => {
  assert.equal(
    worktreePath("/Users/dev/app", "feature/my-change"),
    "/Users/dev/app-feature-my-change",
  );
  assert.equal(
    worktreePath("/Users/dev/app", "sushi/20240101-0930"),
    "/Users/dev/app-sushi-20240101-0930",
  );
});

test("worktreeAddArgs never builds a shell string - always an argv array", () => {
  assert.deepEqual(worktreeAddArgs("feature/x", "/Users/dev/app-feature-x"), [
    "worktree",
    "add",
    "-b",
    "feature/x",
    "/Users/dev/app-feature-x",
    "refs/heads/main",
  ]);
  assert.equal(
    worktreeAddArgs("feature/x", "/tmp/app-x", "origin/main").at(-1),
    "origin/main",
  );
});

test("createWorktree rejects a cwd outside any git repository", async () => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-plain-"),
  );
  await assert.rejects(
    createWorktree(directory, "feature/x"),
    /not a git repository/,
  );
  await fs.rm(directory, { recursive: true, force: true });
});

test("createWorktree rejects an invalid branch before running any git command", async () => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-invalid-branch-"),
  );
  await assert.rejects(
    createWorktree(directory, "-bad"),
    /Branch name cannot start with/,
  );
  await fs.rm(directory, { recursive: true, force: true });
});

test("createWorktree rejects a non-existent cwd", async () => {
  await assert.rejects(
    createWorktree("/Users/dev/does-not-exist-192-0-2-0", "feature/x"),
    /Choose an existing project folder/,
  );
});

test("createWorktree defaults to main and IPC respects an explicit base in a real repo", async (t) => {
  const repo = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-repo-"),
  );
  t.after(() => fs.rm(repo, { recursive: true, force: true }));
  await execute("git", ["-C", repo, "init", "-q", "-b", "main"]);
  await execute("git", [
    "-C",
    repo,
    "config",
    "user.email",
    "test@example.com",
  ]);
  await execute("git", ["-C", repo, "config", "user.name", "Test"]);
  await fs.writeFile(path.join(repo, "README.md"), "hello\n");
  await execute("git", ["-C", repo, "add", "README.md"]);
  await execute("git", ["-C", repo, "commit", "-q", "-m", "initial"]);
  const { stdout: mainCommit } = await execute("git", [
    "-C",
    repo,
    "rev-parse",
    "HEAD",
  ]);
  await execute("git", ["-C", repo, "checkout", "-q", "-b", "feature/current"]);
  await fs.writeFile(path.join(repo, "feature.txt"), "current branch only\n");
  await execute("git", ["-C", repo, "add", "feature.txt"]);
  await execute("git", ["-C", repo, "commit", "-q", "-m", "feature commit"]);

  const { path: worktreeDir } = await createWorktree(repo, "sushi/test-branch");
  t.after(() => fs.rm(worktreeDir, { recursive: true, force: true }));
  // git resolves symlinks (macOS's /tmp -> /private/tmp) before answering
  // rev-parse --show-toplevel, so the expectation is built the same way.
  const realRepo = await fs.realpath(repo);
  assert.equal(
    worktreeDir,
    path.join(
      path.dirname(realRepo),
      `${path.basename(realRepo)}-sushi-test-branch`,
    ),
  );
  const stat = await fs.stat(worktreeDir);
  assert.ok(stat.isDirectory());
  const { stdout: branch } = await execute("git", [
    "-C",
    worktreeDir,
    "rev-parse",
    "--abbrev-ref",
    "HEAD",
  ]);
  assert.equal(branch.trim(), "sushi/test-branch");
  const { stdout: worktreeCommit } = await execute("git", [
    "-C",
    worktreeDir,
    "rev-parse",
    "HEAD",
  ]);
  assert.equal(
    worktreeCommit.trim(),
    mainCommit.trim(),
    "the default base is main, even while the source checkout is on a feature branch",
  );

  // A second attempt at the same branch/path is refused rather than clobbering it.
  await assert.rejects(createWorktree(repo, "sushi/test-branch"));

  const handlers = new Map();
  const connections = new Connections(repo);
  await connections.init();
  t.after(() => connections.close());
  registerProjectIpc({
    handle: (name, handler) => handlers.set(name, handler),
    getConnections: () => connections,
  });
  const { path: chosenDir } = await handlers.get("worktree-create")(
    repo,
    "sushi/chosen-base",
    "feature/current",
  );
  t.after(() => fs.rm(chosenDir, { recursive: true, force: true }));
  const { stdout: chosenCommit } = await execute("git", [
    "-C",
    chosenDir,
    "rev-parse",
    "HEAD",
  ]);
  const { stdout: featureCommit } = await execute("git", [
    "-C",
    repo,
    "rev-parse",
    "feature/current",
  ]);
  assert.equal(chosenCommit.trim(), featureCommit.trim());
  assert.notEqual(chosenCommit.trim(), mainCommit.trim());
});

async function remoteRepository(t) {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-fresh-base-"),
  );
  const remote = path.join(directory, "origin.git");
  const source = path.join(directory, "source");
  const clone = path.join(directory, "clone");
  const git = async (root, ...args) => {
    const { stdout } = await execute("git", [
      "-C",
      root,
      "-c",
      "user.name=Tester",
      "-c",
      "user.email=tester@example.invalid",
      ...args,
    ]);
    return stdout.trim();
  };
  await execute("git", ["init", "--bare", "-q", "-b", "main", remote]);
  await execute("git", ["init", "-q", "-b", "main", source]);
  await fs.writeFile(path.join(source, "README.md"), "initial\n");
  await git(source, "add", "README.md");
  await git(source, "commit", "-qm", "initial");
  await git(source, "remote", "add", "origin", remote);
  await git(source, "push", "-qu", "origin", "main");
  await execute("git", ["clone", "-q", remote, clone]);
  const connections = new Connections(directory);
  await connections.init();
  t.after(async () => {
    await connections.close();
    await fs.rm(directory, { recursive: true, force: true });
  });
  const publish = async (name) => {
    await fs.writeFile(path.join(source, name), name + "\n");
    await git(source, "add", name);
    await git(source, "commit", "-qm", "add " + name);
    await git(source, "push", "-q", "origin", "HEAD");
    return git(source, "rev-parse", "HEAD");
  };
  return { directory, remote, source, clone, connections, git, publish };
}

test("local worktree IPC fetches latest main without moving divergent local or dirty source state", async (t) => {
  const { clone, connections, git, publish } = await remoteRepository(t);
  await git(clone, "commit", "--allow-empty", "-qm", "local-only main");
  const localMain = await git(clone, "rev-parse", "refs/heads/main");
  await git(clone, "checkout", "-qb", "feature/current");
  await fs.writeFile(path.join(clone, "README.md"), "dirty source\n");
  const status = await git(clone, "status", "--porcelain");
  const latest = await publish("latest.txt");
  const handlers = new Map();
  registerProjectIpc({
    handle: (name, handler) => handlers.set(name, handler),
    getConnections: () => connections,
  });
  const created = await handlers.get("worktree-create")(clone, "feature/fresh");
  assert.equal(await git(created.path, "rev-parse", "HEAD"), latest);
  assert.equal(await git(clone, "rev-parse", "refs/heads/main"), localMain);
  assert.equal(
    await git(clone, "symbolic-ref", "--short", "HEAD"),
    "feature/current",
  );
  assert.equal(await git(clone, "status", "--porcelain"), status);
  assert.equal(
    await fs.readFile(path.join(clone, "README.md"), "utf8"),
    "dirty source\n",
  );
});

test("a failed base fetch stops local creation without making a branch or checkout", async (t) => {
  const { directory, clone, connections, git } = await remoteRepository(t);
  await git(
    clone,
    "remote",
    "set-url",
    "origin",
    path.join(directory, "missing.git"),
  );
  const handlers = new Map();
  registerProjectIpc({
    handle: (name, handler) => handlers.set(name, handler),
    getConnections: () => connections,
  });
  assert.throws(
    () =>
      handlers.get("project-inspect")(null, {
        operation: "worktree_base",
        root: clone,
      }),
    /Unknown project operation/,
  );
  await assert.rejects(
    handlers.get("worktree-create")(clone, "feature/fetch-failed"),
    /repository/,
  );
  await assert.rejects(fs.stat(worktreePath(clone, "feature/fetch-failed")), {
    code: "ENOENT",
  });
  await assert.rejects(
    git(clone, "show-ref", "--verify", "refs/heads/feature/fetch-failed"),
  );
  assert.equal(await git(clone, "symbolic-ref", "--short", "HEAD"), "main");
});

test("base preparation honors a configured upstream and fetches explicit remote-only branches", async (t) => {
  const { remote, source, clone, connections, git, publish } =
    await remoteRepository(t);
  await git(source, "checkout", "-qb", "release/stable");
  const release = await publish("stable.txt");
  await git(clone, "remote", "add", "upstream", remote);
  await git(clone, "config", "branch.main.remote", "upstream");
  await git(clone, "config", "branch.main.merge", "refs/heads/release/stable");
  const inspect = (base) =>
    connections.inspect(null, {
      operation: "worktree_base",
      root: clone,
      base,
    });
  assert.equal(
    (await inspect("refs/heads/main")).base,
    release,
    "main follows its configured upstream even when origin/main exists",
  );
  const latest = await publish("stable-latest.txt");
  assert.equal(
    (await inspect("refs/remotes/origin/release/stable")).base,
    latest,
  );
  assert.equal((await inspect("origin/release/stable")).base, latest);
  for (const invalid of [
    "",
    "--upload-pack=bad",
    "main^{commit}",
    "refs/tags/main",
    "HEAD",
    null,
  ])
    await assert.rejects(inspect(invalid));
});

test("main without a local checkout or upstream still fetches origin/main", async (t) => {
  const { clone, connections, git, publish } = await remoteRepository(t);
  await git(clone, "branch", "-m", "main", "develop");
  const latest = await publish("origin-only.txt");
  const prepared = await connections.inspect(null, {
    operation: "worktree_base",
    root: clone,
  });
  assert.equal(prepared.base, latest);
  assert.equal(await git(clone, "symbolic-ref", "--short", "HEAD"), "develop");
  await assert.rejects(git(clone, "show-ref", "--verify", "refs/heads/main"));
});

test("remote branch discovery includes uncached main in a single-branch clone and removes stale remote choices", async (t) => {
  const { directory, remote, source, connections, git } =
    await remoteRepository(t);
  await git(source, "checkout", "-qb", "feature/only");
  await git(source, "commit", "--allow-empty", "-qm", "feature head");
  await git(source, "push", "-q", "origin", "HEAD");
  const single = path.join(directory, "single");
  await execute("git", [
    "clone",
    "-q",
    "--single-branch",
    "--branch",
    "feature/only",
    remote,
    single,
  ]);
  const inspect = (includeRemote) =>
    connections.inspect(null, {
      operation: "branches",
      root: single,
      includeRemote,
    });
  assert.equal(
    (await inspect(false)).branches.some((branch) => branch.name === "main"),
    false,
  );
  const discovered = (await inspect(true)).branches;
  const main = discovered.find((branch) => branch.name === "main");
  assert.equal(main.ref, "origin/main");
  assert.equal(main.remoteOnly, true);
  assert.equal(main.local, false);
  assert.equal(
    discovered.find((branch) => branch.current).name,
    "feature/only",
  );
  await assert.rejects(
    git(single, "show-ref", "--verify", "refs/remotes/origin/main"),
    "discovery must not fetch or create tracking refs",
  );
  const feature = await git(single, "rev-parse", "HEAD");
  await git(single, "update-ref", "refs/remotes/origin/stale", feature);
  assert.equal(
    (await inspect(false)).branches.some((branch) => branch.name === "stale"),
    true,
  );
  assert.equal(
    (await inspect(true)).branches.some((branch) => branch.name === "stale"),
    false,
  );
  await git(
    single,
    "remote",
    "set-url",
    "origin",
    path.join(directory, "missing.git"),
  );
  await assert.rejects(inspect(true), /repository/);
  assert.equal(
    (await inspect(false)).branches.some((branch) => branch.name === "stale"),
    true,
    "ordinary branch inspection keeps its cached behavior",
  );
});

test("local creation from a linked checkout uses the primary repository and fresh main", async (t) => {
  const { clone, connections, git, publish } = await remoteRepository(t);
  const linked = await createWorktree(clone, "feature/existing");
  const latest = await publish("linked-latest.txt");
  const handlers = new Map();
  registerProjectIpc({
    handle: (name, handler) => handlers.set(name, handler),
    getConnections: () => connections,
  });
  const created = await handlers.get("worktree-create")(
    linked.path,
    "feature/from-linked",
  );
  assert.equal(created.root, await fs.realpath(clone));
  assert.equal(
    created.path,
    worktreePath(await fs.realpath(clone), "feature/from-linked"),
  );
  assert.equal(await git(created.path, "rev-parse", "HEAD"), latest);
  assert.equal(
    await git(linked.path, "symbolic-ref", "--short", "HEAD"),
    "feature/existing",
  );
});

test("Herdr creation prepares the selected SSH target's upstream and primary cwd before sending RPC", async (t) => {
  const { directory, clone, connections, git, publish } =
    await remoteRepository(t);
  const linked = await createWorktree(clone, "feature/existing");
  const latest = await publish("ssh-latest.txt");
  const fakeSsh = path.join(directory, "ssh");
  await fs.writeFile(
    fakeSsh,
    "#!/usr/bin/env node\nconst { spawn } = require('node:child_process');\nconst child = spawn('/bin/sh', ['-c', process.argv.at(-1)], { stdio: 'inherit' });\nchild.on('exit', code => process.exit(code));\n",
    { mode: 0o755 },
  );
  connections.ssh = fakeSsh;
  const profile = await connections.save({
    name: "Lab",
    host: "user@devbox",
    socket: "/tmp/example.sock",
  });
  const endpoint = "ssh:" + profile.id;
  const socketPath = path.join(directory, "gateway.sock");
  const calls = [];
  const server = net.createServer((client) => {
    let buffer = "";
    client.on("data", (chunk) => {
      buffer += chunk;
      if (!buffer.includes("\n")) return;
      const message = JSON.parse(buffer.trim());
      calls.push(message);
      client.end(
        JSON.stringify({ id: message.id, result: { type: "ok" } }) + "\n",
      );
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const handlers = new Map();
  registerHerdrExtension({
    handle: (name, handler) => handlers.set(name, handler),
    getConnections: () => ({
      socket: async (selected) => {
        assert.equal(selected, endpoint);
        return socketPath;
      },
      inspect: (selected, options) => connections.inspect(selected, options),
    }),
    id: (value) => value,
  });
  await handlers.get("herdr")(endpoint, "worktree.create", {
    cwd: linked.path,
    branch: "feature/ssh-fresh",
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].method, "worktree.create");
  assert.equal(calls[0].params.cwd, await fs.realpath(clone));
  assert.equal(calls[0].params.base, latest);
  assert.equal(connections.inspectionWorkers.has(endpoint), true);
  assert.equal(connections.inspectionWorkers.has("local"), false);
  await git(
    clone,
    "remote",
    "set-url",
    "origin",
    path.join(directory, "missing.git"),
  );
  await assert.rejects(
    handlers.get("herdr")(endpoint, "worktree.create", {
      cwd: linked.path,
      branch: "feature/failed",
    }),
    /repository/,
  );
  assert.equal(calls.length, 1, "a failed fetch sends no worktree.create RPC");
});

test("an unavailable main fails without creating a checkout or branch", async (t) => {
  const repo = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-no-main-"),
  );
  t.after(() => fs.rm(repo, { recursive: true, force: true }));
  await execute("git", ["-C", repo, "init", "-q", "-b", "develop"]);
  await execute("git", [
    "-C",
    repo,
    "-c",
    "user.name=Test",
    "-c",
    "user.email=test@example.com",
    "commit",
    "--allow-empty",
    "-q",
    "-m",
    "initial",
  ]);
  await assert.rejects(createWorktree(repo, "sushi/no-main"), /main/);
  const target = worktreePath(await fs.realpath(repo), "sushi/no-main");
  await assert.rejects(fs.stat(target), { code: "ENOENT" });
  await assert.rejects(
    execute("git", [
      "-C",
      repo,
      "show-ref",
      "--verify",
      "refs/heads/sushi/no-main",
    ]),
  );
});

test("only a terminal or an agent launches in a worktree", () => {
  assert.equal(launchesInWorktree("terminal"), true);
  assert.equal(launchesInWorktree("agent"), true);
  for (const kind of ["files", "browser", "chat", "extension"])
    assert.equal(
      launchesInWorktree(kind),
      false,
      `${kind} is a view of the project, not a session`,
    );
});

test("the checkout itself runs without a timeout and reports the repo root", async () => {
  const calls = [];
  const execFn = async (file, args, options) => {
    calls.push({ args, timeout: options.timeout });
    return args.includes("rev-parse")
      ? { stdout: "/Users/dev/app\n" }
      : { stdout: "" };
  };
  const result = await createWorktree(
    "/Users/dev/app/service",
    "feature/x",
    "main",
    {
      execFn,
      // Only the project folder exists: the worktree path must look free.
      stat: (target) => {
        if (target !== "/Users/dev/app/service")
          throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
        return { isDirectory: () => true, isFile: () => false };
      },
    },
  );
  assert.equal(result.root, "/Users/dev/app");
  assert.equal(result.path, "/Users/dev/app-feature-x");
  const query = calls.find((call) => call.args.includes("rev-parse"));
  const add = calls.find((call) => call.args.includes("add"));
  assert.equal(query.timeout, 15000, "a query may be cut short");
  assert.equal(add.timeout, 0, "killing the checkout would strand a branch");
});
