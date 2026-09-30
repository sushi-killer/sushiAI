const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
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
      label: "feature/x",
      focus: false,
    },
  );
});

test("herdrWorkspaceKey builds the sushiAI id for local and SSH endpoints", () => {
  assert.equal(
    herdrWorkspaceKey("unix:///tmp/herdr.sock", "ws-1"),
    "herdr:local:ws-1",
  );
  assert.equal(
    herdrWorkspaceKey("ssh:devbox", "ws-2"),
    "herdr:ssh:devbox:ws-2",
  );
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
    getConnections: () => ({ socket: async () => socketPath }),
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
  ]);
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

// The only test in this file that runs real git - against a throwaway repo
// this test creates under os.tmpdir() and removes afterwards. No Herdr
// process and no network are involved.
test("createWorktree adds a linked worktree on a new branch next to a real repo", async (t) => {
  const repo = await fs.mkdtemp(
    path.join(os.tmpdir(), "worktree-session-repo-"),
  );
  t.after(() => fs.rm(repo, { recursive: true, force: true }));
  await execute("git", ["-C", repo, "init", "-q"]);
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

  // A second attempt at the same branch/path is refused rather than clobbering it.
  await assert.rejects(createWorktree(repo, "sushi/test-branch"));
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
  const result = await createWorktree("/Users/dev/app/service", "feature/x", {
    execFn,
    // Only the project folder exists: the worktree path must look free.
    stat: (target) => {
      if (target !== "/Users/dev/app/service")
        throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return { isDirectory: () => true, isFile: () => false };
    },
  });
  assert.equal(result.root, "/Users/dev/app");
  assert.equal(result.path, "/Users/dev/app-feature-x");
  const query = calls.find((call) => call.args.includes("rev-parse"));
  const add = calls.find((call) => call.args.includes("add"));
  assert.equal(query.timeout, 15000, "a query may be cut short");
  assert.equal(add.timeout, 0, "killing the checkout would strand a branch");
});
