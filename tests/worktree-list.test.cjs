const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  worktreeListScript,
  parseWorktreeList,
  worktreeRemoveScript,
} = require("../electron/worktree.cjs");

// No git identity reaches the scripts, as on a CI runner or a fresh host:
// HOME points nowhere and the global config is off.
const sh = (script) =>
  execFileSync("sh", ["-c", script], {
    env: {
      ...process.env,
      HOME: os.tmpdir(),
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
      // and git may not invent one from the host name, as it does on macOS.
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "user.useConfigOnly",
      GIT_CONFIG_VALUE_0: "true",
    },
  }).toString();

function git(cwd, ...args) {
  execFileSync("git", ["-C", cwd, ...args], {
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "t",
      GIT_AUTHOR_EMAIL: "t@example.com",
      GIT_COMMITTER_NAME: "t",
      GIT_COMMITTER_EMAIL: "t@example.com",
    },
    stdio: "pipe",
  });
}

/** A repository on `main` with four linked worktrees: one squash-merged, one
 * with an unmerged commit and an uncommitted file, one whose folder was
 * deleted by hand, and one whose branch was merged normally. */
function fixture() {
  const dir = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-wt-")),
  );
  const repo = path.join(dir, "repo");
  fs.mkdirSync(repo);
  git(repo, "init", "-q", "-b", "main");
  git(repo, "commit", "-q", "--allow-empty", "-m", "init");
  const file = (where, name, text) =>
    fs.writeFileSync(path.join(where, name), text);
  const add = (branch) => {
    const where = path.join(dir, `repo-${branch}`);
    git(repo, "worktree", "add", "-q", "-b", branch, where);
    return where;
  };
  const squashed = add("squashed");
  file(squashed, "s", "1");
  git(squashed, "add", "s");
  git(squashed, "commit", "-q", "-m", "s1");
  file(squashed, "s", "2");
  git(squashed, "commit", "-q", "-am", "s2");
  git(repo, "merge", "--squash", "-q", "squashed");
  git(repo, "commit", "-q", "-m", "squash");
  const open = add("open's");
  file(open, "o", "1");
  git(open, "add", "o");
  git(open, "commit", "-q", "-m", "o");
  file(open, "dirty", "1");
  const gone = add("gone");
  fs.rmSync(gone, { recursive: true });
  const merged = add("merged");
  file(merged, "m", "1");
  git(merged, "add", "m");
  git(merged, "commit", "-q", "-m", "m");
  git(repo, "merge", "-q", "--no-edit", "merged");
  return { dir, repo, squashed, open, gone, merged };
}

test("lists every worktree with its merge state, squash merges included", (t) => {
  const f = fixture();
  t.after(() => fs.rmSync(f.dir, { recursive: true, force: true }));
  const list = parseWorktreeList(sh(worktreeListScript(f.open)));
  assert.equal(list.root, f.repo);
  assert.equal(list.base, "main");
  const by = Object.fromEntries(list.worktrees.map((w) => [w.branch, w]));
  assert.equal(list.worktrees[0].path, f.repo);
  assert.equal(list.worktrees[0].main, true);
  assert.equal(by.squashed.merged, true);
  assert.equal(by.squashed.main, false);
  assert.equal(by.merged.merged, true);
  assert.deepEqual(
    [by["open's"].merged, by["open's"].ahead, by["open's"].changes],
    [false, 1, 1],
  );
  assert.equal(by["open's"].exists, true);
  assert.equal(by.gone.exists, false);
  assert.ok(by.squashed.committedAt > 0);
});

test("removes a worktree with its changes, or forgets one whose folder is gone", (t) => {
  const f = fixture();
  t.after(() => fs.rmSync(f.dir, { recursive: true, force: true }));
  sh(worktreeRemoveScript(f.repo, f.open, "open's"));
  sh(worktreeRemoveScript(f.repo, f.gone, ""));
  const list = parseWorktreeList(sh(worktreeListScript(f.repo)));
  const branches = list.worktrees.map((w) => w.branch).sort();
  assert.deepEqual(branches, ["main", "merged", "squashed"]);
  assert.equal(fs.existsSync(f.open), false);
  assert.throws(() =>
    git(f.repo, "rev-parse", "--verify", "refs/heads/open's"),
  );
  assert.doesNotThrow(() =>
    git(f.repo, "rev-parse", "--verify", "refs/heads/gone"),
  );
});

test("a missing folder or a folder outside git is an error, not an empty list", () => {
  assert.throws(
    () => parseWorktreeList(sh(worktreeListScript("/nonexistent/sushiai"))),
    /gone/,
  );
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-nogit-"));
  try {
    assert.throws(
      () => parseWorktreeList(sh(worktreeListScript(dir))),
      /not a git repository/,
    );
  } finally {
    fs.rmSync(dir, { recursive: true });
  }
});

test("rows join every host's worktrees with open sessions and orchd tasks", async () => {
  const { worktreeRows, removable, removalLoss } =
    await import("../src/projectWorktrees.ts");
  const wt = (path, extra = {}) => ({
    path,
    branch: path.split("/").pop(),
    head: "abc1234",
    committedAt: 1,
    exists: true,
    changes: 0,
    merged: false,
    ahead: 0,
    main: false,
    ...extra,
  });
  const lists = [
    {
      host: "ssh:devbox",
      cwd: "/home/user/app",
      root: "/home/user/app",
      base: "origin/main",
      worktrees: [wt("/home/user/app", { main: true }), wt("/home/user/app-b")],
    },
    {
      host: "local",
      cwd: "/w/app",
      root: "/w/app",
      base: "main",
      worktrees: [
        wt("/w/app", { main: true }),
        wt("/w/app-open", { changes: 3, ahead: 2 }),
        wt("/w/app-task", { merged: true }),
      ],
    },
    { host: "ssh:lab", cwd: "/x", error: "Connection timed out" },
  ];
  const workspaces = [
    { id: "1", cwd: "/w/app-open/src", panels: [{}, {}] },
    { id: "2", cwd: "/w/app-opener", panels: [{}] },
    { id: "3", cwd: "/home/user/app-b", connection: "ssh:other", panels: [] },
  ];
  const tasks = [
    { title: "Retry", worktree: "/w/app-task", status: "running" },
  ];
  const profiles = [{ id: "devbox", name: "devbox", host: "user@devbox" }];
  const rows = worktreeRows(lists, workspaces, tasks, profiles);
  assert.deepEqual(
    rows.map((row) => `${row.hostLabel} ${row.worktree.path}`),
    [
      "This Mac /w/app",
      "This Mac /w/app-open",
      "This Mac /w/app-task",
      "devbox /home/user/app",
      "devbox /home/user/app-b",
    ],
  );
  const [main, open, task, , remote] = rows;
  assert.deepEqual(
    open.open.map((w) => w.id),
    ["1"],
  );
  assert.deepEqual(remote.open, []);
  assert.equal(remote.base, "main");
  assert.equal(removable(main), false);
  assert.equal(removable(task), false);
  assert.equal(
    removable({ ...task, task: { ...task.task, status: "done" } }),
    true,
  );
  assert.equal(
    removalLoss(open),
    "On This Mac, the folder app-open is deleted with its 3 uncommitted changes, and its 2 open sessions stop. The branch has 2 commits that are not in main.",
  );
  assert.equal(
    removalLoss({ ...remote, worktree: { ...remote.worktree, exists: false } }),
    "On devbox, the folder app-b is already gone; the worktree is forgotten.",
  );
});

test("the base branch, a fresh worktree and a locked one are never merged or removable", async (t) => {
  const dir = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "sushiai-wt-")),
  );
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const repo = path.join(dir, "repo");
  fs.mkdirSync(repo);
  git(repo, "init", "-q", "-b", "main");
  git(repo, "commit", "-q", "--allow-empty", "-m", "init");
  git(repo, "checkout", "-q", "-b", "feat");
  const onMain = path.join(dir, "repo-main");
  git(repo, "worktree", "add", "-q", onMain, "main");
  const fresh = path.join(dir, "repo-fresh");
  git(repo, "worktree", "add", "-q", "-b", "fresh", fresh, "main");
  const locked = path.join(dir, "repo-locked");
  git(repo, "worktree", "add", "-q", "-b", "locked", locked, "main");
  git(repo, "worktree", "lock", locked);
  const list = parseWorktreeList(sh(worktreeListScript(repo)));
  const by = Object.fromEntries(list.worktrees.map((w) => [w.branch, w]));
  assert.equal(by.main.merged, false);
  assert.equal(by.fresh.merged, false);
  assert.equal(by.locked.locked, true);

  // The IPC handler, with a real repository behind it.
  const { registerProjectIpc } = require("../electron/ipc/projects.cjs");
  const handlers = {};
  registerProjectIpc({
    handle: (name, fn) => (handlers[name] = fn),
    getConnections: () => null,
    projects: { folders: async () => [] },
  });
  const remove = handlers["worktrees:remove"];
  await assert.rejects(remove("local", repo, repo, true), /not part/);
  await assert.rejects(remove("local", repo, dir, true), /not part/);
  await assert.rejects(remove("local", repo, locked, true), /locked/);
  await remove("local", repo, onMain, true);
  assert.equal(fs.existsSync(onMain), false);
  assert.doesNotThrow(() =>
    git(repo, "rev-parse", "--verify", "refs/heads/main"),
  );
  const lists = await handlers["worktrees:list"](null, "local", fresh);
  assert.equal(lists.length, 1);
  assert.equal(lists[0].root, repo);
});

test("an unfinished task wins over a finished one on the same worktree", async () => {
  const { worktreeRows, removable } =
    await import("../src/projectWorktrees.ts");
  const worktree = {
    path: "/w/app-t",
    branch: "task/7",
    head: "a",
    committedAt: 1,
    exists: true,
    changes: 0,
    merged: true,
    ahead: 0,
    locked: false,
    main: false,
  };
  const lists = [
    {
      host: "local",
      cwd: "/w/app",
      root: "/w/app",
      base: "main",
      worktrees: [worktree],
    },
  ];
  const [byPath] = worktreeRows(
    lists,
    [],
    [
      {
        title: "old",
        worktree: "/w/app-t/",
        branch: "",
        repo: "/w/app",
        status: "done",
      },
      {
        title: "retry",
        worktree: "",
        branch: "task/7",
        repo: "/w/app",
        status: "running",
      },
    ],
    [],
  );
  assert.equal(byPath.task.title, "retry");
  assert.equal(removable(byPath), false);
});

test("closing a worktree's last session keeps a changed checkout, a moved branch and the branch itself", async (t) => {
  const f = fixture();
  t.after(() => fs.rmSync(f.dir, { recursive: true, force: true }));
  const { registerProjectIpc } = require("../electron/ipc/projects.cjs");
  const handlers = {};
  registerProjectIpc({
    handle: (name, fn) => (handlers[name] = fn),
    getConnections: () => null,
    projects: { folders: async () => [] },
  });
  const remove = (target, options) =>
    handlers["worktrees:remove"]("local", target, target, options);
  await assert.rejects(remove(f.repo, { branch: "main" }), /not part/);
  await assert.rejects(remove(f.merged, { branch: "other" }), /changed/);
  await assert.rejects(remove(f.open, { branch: "open's" }), /uncommitted/);
  assert.equal(fs.existsSync(path.join(f.open, "dirty")), true);
  // Changed after the owner looked, with untracked files hidden from
  // `git status`: the removal script itself checks again and keeps it.
  git(f.repo, "config", "status.showUntrackedFiles", "no");
  fs.writeFileSync(path.join(f.merged, "late"), "1");
  assert.throws(
    () => sh(worktreeRemoveScript(f.repo, f.merged, "", false)),
    /uncommitted changes/,
  );
  assert.equal(fs.existsSync(path.join(f.merged, "late")), true);
  await assert.rejects(remove(f.merged, { branch: "merged" }), /uncommitted/);
  fs.rmSync(path.join(f.merged, "late"));
  await assert.rejects(
    remove(f.merged, { branch: "merged", head: "0".repeat(40) }),
    /changed/,
  );
  assert.deepEqual(await remove(f.merged, { branch: "merged" }), {
    removed: f.merged,
    branch: "merged",
  });
  assert.equal(fs.existsSync(f.merged), false);
  assert.doesNotThrow(() =>
    git(f.repo, "rev-parse", "--verify", "refs/heads/merged"),
  );
});
