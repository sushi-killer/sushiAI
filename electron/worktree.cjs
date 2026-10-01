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

const quote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;

// A non-interactive ssh shell often lacks the user's tool directories (as in
// orchestrator-remote.cjs).
const PATH_SH =
  'export PATH="$HOME/.local/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"';

/** Lists every worktree of the repository holding `cwd`, one `WT` line each
 * (path, branch, head, last commit time, folder exists, changed files,
 * merged, commits ahead, locked), after a `ROOT` line naming the base
 * branch. Plain sh and git, so the same script runs on this Mac and over
 * ssh. Merged means the head is in the base, or a commit with the same tree
 * on the merge base is (a squash merge); `commit-tree` only writes a
 * dangling object for that check. The base branch itself, and a branch that
 * never had a commit of its own (its reflog holds only its creation), are
 * never merged: removing them as merged would delete the base or a worktree
 * just cut. */
function worktreeListScript(cwd) {
  return `${PATH_SH}
cd ${quote(cwd)} 2>/dev/null || { echo NOFOLDER; exit 0; }
git rev-parse --show-toplevel >/dev/null 2>&1 || { echo NOREPO; exit 0; }
base=$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null)
if [ -z "$base" ]; then for b in main master; do
  if git rev-parse -q --verify "refs/heads/$b" >/dev/null; then base=$b; break; fi
done; fi
printf 'ROOT\t\t%s\n' "$base"
emit() {
  [ -n "$p" ] || return 0
  t=$(git log -1 --format=%ct "$h" 2>/dev/null)
  e=0; d=0; m=0; a=0
  if [ -d "$p" ]; then e=1; d=$(git -C "$p" status --porcelain 2>/dev/null | wc -l | tr -d ' '); fi
  own=1
  if [ -n "$br" ]; then
    [ "$br" = "\${base#origin/}" ] && own=0
    [ "$(git reflog show --format=%H "refs/heads/$br" 2>/dev/null | wc -l | tr -d ' ')" -le 1 ] && own=0
  fi
  if [ -n "$base" ] && [ -n "$h" ]; then
    if git merge-base --is-ancestor "$h" "$base" 2>/dev/null; then m=$own
    else
      a=$(git rev-list --count "$base..$h" 2>/dev/null || echo 0)
      mb=$(git merge-base "$base" "$h" 2>/dev/null)
      if [ "$own" = 1 ] && [ -n "$mb" ] && c=$(git commit-tree "$h^{tree}" -p "$mb" -m squash 2>/dev/null); then
        case $(git cherry "$base" "$c" 2>/dev/null) in -*) m=1 ;; esac
      fi
    fi
  fi
  printf 'WT\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$p" "$br" "$h" "\${t:-0}" "$e" "$d" "$m" "$a" "$l"
}
p=; h=; br=; l=0
git worktree list --porcelain | { while IFS= read -r line; do
  case $line in
    "worktree "*) p=\${line#worktree } ;;
    "HEAD "*) h=\${line#HEAD } ;;
    "branch "*) br=\${line#branch refs/heads/} ;;
    locked|"locked "*) l=1 ;;
    "") emit; p=; h=; br=; l=0 ;;
  esac
done; emit; }
`;
}

/** The output of `worktreeListScript`: the repository, its base branch and
 * its worktrees, main checkout first. Throws on a folder that is gone or is
 * not a git repository. */
function parseWorktreeList(output) {
  const lines = String(output || "").split("\n");
  if (lines.includes("NOFOLDER"))
    throw new Error("The project folder is gone.");
  if (lines.includes("NOREPO"))
    throw new Error("The project folder is not a git repository.");
  const head = lines.find((line) => line.startsWith("ROOT\t"));
  if (!head) throw new Error("Could not list worktrees.");
  const base = head.split("\t")[2];
  const worktrees = lines
    .filter((line) => line.startsWith("WT\t"))
    .map((line) => {
      const [
        ,
        path,
        branch,
        head,
        time,
        exists,
        changes,
        merged,
        ahead,
        locked,
      ] = line.split("\t");
      return {
        path,
        branch,
        head,
        committedAt: Number(time) * 1000 || 0,
        exists: exists === "1",
        changes: Number(changes) || 0,
        merged: merged === "1",
        ahead: Number(ahead) || 0,
        locked: locked === "1",
      };
    })
    .map((item, index) => ({ ...item, main: index === 0 }));
  // The main checkout, not `cwd`'s own top level: that is the linked
  // worktree itself when the project folder is one.
  return { root: worktrees[0]?.path || "", base: base || "", worktrees };
}

/** Removes one linked worktree: its folder (with its changes only when
 * `discardChanges` - the owner confirmed losing them), or only git's record of that one worktree when its folder is
 * already gone (never `prune`, which would also forget a worktree on a drive
 * that is merely unmounted), then its branch when asked. */
function worktreeRemoveScript(root, path, branch, discardChanges = true) {
  const drop = branch ? ` && git branch -D ${quote(branch)}` : "";
  // Without --force git itself refuses a checkout with modified or untracked
  // files, so a change made after the caller looked is never lost.
  const force = discardChanges ? " --force" : "";
  return `${PATH_SH}
cd ${quote(root)} || exit 1
if [ -d ${quote(path)} ]; then git worktree remove${force} ${quote(path)} || exit 1
else
  admin=$(git rev-parse --git-common-dir)/worktrees
  for d in "$admin"/*; do
    if [ "$(cat "$d/gitdir" 2>/dev/null)" = ${quote(`${path}/.git`)} ]; then rm -rf "$d"; fi
  done
fi${drop}
`;
}

module.exports = {
  worktreeListScript,
  parseWorktreeList,
  worktreeRemoveScript,
  worktreeBranchError,
  worktreePath,
  worktreeAddArgs,
  createWorktree,
};
