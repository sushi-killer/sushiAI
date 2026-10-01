// Creates a linked git worktree on a new branch, next to the repository that
// holds a project's working directory. Used for the "New worktree" launch
// choice on local (non-Herdr) sessions - the Herdr-backed equivalent goes
// through the `worktree.create` socket RPC instead (builtin-herdr.cjs).
const fs = require("node:fs");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const execute = promisify(execFile);

const FORBIDDEN_SEQUENCES = [
  "..",
  "~",
  "^",
  ":",
  "?",
  "*",
  "[",
  "\\",
  "@{",
  "//",
];

/** Mirrors `worktreeBranchError` in src/workspace/worktree.ts. Duplicated
 * rather than shared: electron code never imports a .ts source. Keep the two
 * rule sets in sync by hand when either changes. */
function hasWhitespaceOrControlChar(name) {
  if (/\s/.test(name)) return true;
  for (let i = 0; i < name.length; i++)
    if (name.charCodeAt(i) <= 0x1f) return true;
  return false;
}

function worktreeBranchError(name) {
  if (typeof name !== "string" || !name) return "Branch name is required.";
  if (name.length > 100) return "Branch name is too long.";
  if (hasWhitespaceOrControlChar(name))
    return "Branch name cannot contain whitespace.";
  for (const sequence of FORBIDDEN_SEQUENCES)
    if (name.includes(sequence))
      return `Branch name cannot contain "${sequence}".`;
  if (name.startsWith("-")) return 'Branch name cannot start with "-".';
  if (name.startsWith("/")) return 'Branch name cannot start with "/".';
  if (name.endsWith("/")) return 'Branch name cannot end with "/".';
  if (name.endsWith(".")) return 'Branch name cannot end with ".".';
  if (name.endsWith(".lock")) return 'Branch name cannot end with ".lock".';
  return "";
}

/** Where a new worktree for `branch` lands: a sibling of the repo root, so a
 * checkout is never nested inside the one it was cut from. */
function worktreePath(root, branch) {
  const slug = branch.replace(/\//g, "-");
  return path.join(path.dirname(root), `${path.basename(root)}-${slug}`);
}

function worktreeAddArgs(branch, targetPath, base = "refs/heads/main") {
  return ["worktree", "add", "-b", branch, targetPath, base];
}

function firstLine(text) {
  return String(text || "")
    .trim()
    .split("\n")[0];
}

function isExistingDirectory(value, stat) {
  try {
    return stat(value).isDirectory();
  } catch {
    return false;
  }
}

function pathExists(value, stat) {
  try {
    stat(value);
    return true;
  } catch {
    return false;
  }
}

async function git(root, args, execFn, timeout = 15000) {
  try {
    const { stdout } = await execFn("git", ["-C", root, ...args], {
      timeout,
      maxBuffer: 1024 * 1024,
    });
    return stdout.trim();
  } catch (error) {
    throw new Error(firstLine(error.stderr || error.message) || "git failed");
  }
}

/** Creates a linked worktree for `branch` next to the repository holding
 * `cwd`, on this machine. `execFn` always receives an argv array (never a
 * shell string), so a branch name cannot inject anything into the command
 * line. Resolves to the new checkout's absolute path. */
async function createWorktree(
  cwd,
  branch,
  base = "refs/heads/main",
  { execFn = execute, stat = fs.statSync } = {},
) {
  if (
    typeof cwd !== "string" ||
    !path.isAbsolute(cwd) ||
    !isExistingDirectory(cwd, stat)
  )
    throw new Error("Choose an existing project folder.");
  const branchError = worktreeBranchError(branch);
  if (branchError) throw new Error(branchError);
  if (typeof base !== "string" || !base || base.startsWith("-"))
    throw new Error("Choose an existing base branch.");
  let root;
  try {
    root = await git(cwd, ["rev-parse", "--show-toplevel"], execFn);
  } catch {
    throw new Error("This folder is not a git repository.");
  }
  const targetPath = worktreePath(root, branch);
  if (pathExists(targetPath, stat))
    throw new Error("A worktree already exists at that path.");
  // No timeout on the checkout itself: killing `git worktree add` half way
  // leaves a branch and a partial directory that nothing in the app can
  // remove. A query can hang and be cut short; the write must be allowed to
  // finish. The root travels back so the caller can say which repository the
  // worktree was cut from - `rev-parse` walks upward, so a project folder
  // inside an outer repository resolves to that outer one.
  await git(root, worktreeAddArgs(branch, targetPath, base), execFn, 0);
  return { path: targetPath, root };
}

module.exports = {
  worktreeBranchError,
  worktreePath,
  worktreeAddArgs,
  createWorktree,
};
