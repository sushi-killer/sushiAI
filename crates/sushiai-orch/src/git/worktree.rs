use crate::git::branches::branch_exists;
use crate::git::command::run;
use crate::git::GitError;
use std::path::Path;
use std::path::PathBuf;

/// The `worktreeRoot` value older installs stored as the default. It counts as
/// empty: worktrees no longer live inside the repo by default.
const LEGACY_WORKTREE_ROOT: &str = ".sushiai/worktrees";

/// `<repo folder name>-<8 hex digits>`: the folder name plus a stable FNV-1a
/// hash of the full repo-root path, so two repos with one name never share.
fn repo_id(repo_root: &Path) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for b in repo_root.to_string_lossy().bytes() {
        hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    let name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    format!("{name}-{hash:08x}")
}

/// Where a task's worktree goes: `<root>/<branch with / -> ->`. An empty
/// `root` (the `worktreeRoot` setting, or the legacy stored default) means
/// `<home>/worktrees.noindex/<repo-id>`; the `.noindex` suffix keeps Spotlight
/// from indexing worktrees, `node_modules` and build output. Any other value
/// is resolved against the repo root when relative.
pub fn worktree_path(repo_root: &Path, home: &Path, root: &str, branch: &str) -> PathBuf {
    let root = root.trim();
    let base = if root.is_empty() || root == LEGACY_WORKTREE_ROOT {
        home.join("worktrees.noindex").join(repo_id(repo_root))
    } else {
        repo_root.join(root)
    };
    base.join(branch.replace('/', "-"))
}

/// Makes the repo ignore the whole relative `worktree_root` (`/src/.wt/` for
/// `src/.wt`, never all of `src/`) through `.git/info/exclude`, never a tracked
/// file. A root outside the repo needs nothing.
pub fn exclude_worktree_root(repo_root: &Path, worktree_root: &Path) -> std::io::Result<()> {
    let Ok(rel) = worktree_root.strip_prefix(repo_root) else {
        return Ok(());
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return Ok(());
    }
    let entry = format!("/{}/", parts.join("/"));
    let out = match run(repo_root, &["rev-parse", "--git-path", "info/exclude"]) {
        Ok(out) => out,
        Err(_) => return Ok(()),
    };
    let exclude_path = repo_root.join(out.trim());
    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)?;
    use std::io::Write;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(f)?;
    }
    writeln!(f, "{entry}")
}

pub struct CreatedWorktree {
    pub path: PathBuf,
    pub base_sha: String,
}

/// `git worktree add --detach <path> <sha>`: a throwaway checkout of one
/// commit, on no branch.
pub fn add_detached_worktree(repo_root: &Path, path: &Path, sha: &str) -> Result<(), GitError> {
    let path = path
        .to_str()
        .ok_or_else(|| GitError("non-utf8 path".into()))?;
    run(repo_root, &["worktree", "add", "--detach", path, sha]).map(|_| ())
}

/// `git worktree remove --force <path>`, then `git worktree prune`. Both are
/// best effort: the caller is cleaning up and has nothing left to do on error.
pub fn remove_worktree(repo_root: &Path, path: &Path) {
    crate::procs::stop_processes_under(path);
    if let Some(p) = path.to_str() {
        let _ = run(repo_root, &["worktree", "remove", "--force", p]);
    }
    let _ = run(repo_root, &["worktree", "prune"]);
}

/// `git worktree add -b <branch> <path> HEAD`, then records `baseSha`.
/// `base` is any commit-ish (`HEAD`, a branch, a sha): the new branch
/// starts there, and `base_sha` is that exact commit, resolved once up
/// front so a checkout moving meanwhile can't skew the diff base.
pub fn create_worktree(
    repo_root: &Path,
    branch: &str,
    path: &Path,
    base: &str,
) -> Result<CreatedWorktree, GitError> {
    let base_sha = run(
        repo_root,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?
    .trim()
    .to_string();
    let branch_existed = branch_exists(repo_root, branch);
    let added = run(
        repo_root,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str()
                .ok_or_else(|| GitError("non-utf8 path".into()))?,
            &base_sha,
        ],
    );
    if let Err(e) = added {
        // `worktree add -b` creates the branch before it checks the path,
        // so a taken path would otherwise leave a stray branch behind.
        if !branch_existed {
            let _ = run(repo_root, &["branch", "-D", branch]);
        }
        return Err(e);
    }
    Ok(CreatedWorktree {
        path: path.to_path_buf(),
        base_sha,
    })
}

// -- worktree cleanup and restore ------------------------------------------

/// Bytes under `path`, counting symlinks as themselves (never followed).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|e| dir_size(&e.path())).sum()
}

pub fn prune_worktrees(repo_root: &Path) {
    let _ = run(repo_root, &["worktree", "prune"]);
}

/// Every linked worktree of the repo (not the main checkout), from
/// `git worktree list --porcelain`.
pub fn linked_worktrees(repo_root: &Path) -> Vec<PathBuf> {
    let out = run(repo_root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    out.lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .skip(1)
        .collect()
}

#[cfg(test)]
#[path = "tests/worktree.rs"]
mod tests;
