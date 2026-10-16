use crate::git::command::{exit_code, non_empty_lines, run, run_bytes, succeeds};
use crate::git::GitError;
use std::path::Path;
use std::path::PathBuf;

/// Reads a committed file; absence is distinct from an unreadable revision.
pub fn show_file(cwd: &Path, rev: &str, path: &str) -> Result<Option<String>, GitError> {
    let tree = run(cwd, &["rev-parse", "--verify", &format!("{rev}^{{tree}}")])?;
    let object = format!("{}:{path}", tree.trim());
    if !succeeds(cwd, &["cat-file", "-e", &object]) {
        return Ok(None);
    }
    run(cwd, &["show", &object]).map(Some)
}

/// `git rev-parse --show-toplevel` -- `task.create`'s `repo` param can be
/// any path inside the checkout (or a symlinked/relative one); this
/// resolves it to the actual repo root before anything else touches it.
pub fn repo_toplevel(path: &Path) -> Result<PathBuf, GitError> {
    let out = run(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// The commit `rev` names (`git rev-parse --verify <rev>^{commit}`), or the
/// git error saying why it does not resolve.
pub fn resolve_commit(repo_root: &Path, rev: &str) -> Result<String, GitError> {
    run(
        repo_root,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )
    .map(|s| s.trim().to_string())
}

/// The commits reachable from HEAD whose message carries `Task-Id: <task_id>`,
/// newest first (at most 300).
pub fn commits_with_task_id(cwd: &Path, task_id: &str) -> Vec<String> {
    run(
        cwd,
        &[
            "log",
            "-n",
            "300",
            "--format=%H",
            "--grep",
            &format!("^Task-Id: {task_id}$"),
            "HEAD",
        ],
    )
    .map(|out| out.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

/// The best common ancestor of two revisions.
pub fn merge_base_of(repo_root: &Path, a: &str, b: &str) -> Result<String, GitError> {
    run(repo_root, &["merge-base", a, b]).map(|s| s.trim().to_string())
}

/// Patch-equivalent commits on `branch` that are not on `base`.
pub fn cherry_count(repo_root: &Path, base: &str, branch: &str) -> Result<u32, GitError> {
    let output = run(repo_root, &["cherry", base, branch])?;
    Ok(output.lines().filter(|line| line.starts_with('+')).count() as u32)
}

pub fn head_sha(worktree: &Path) -> Result<String, GitError> {
    Ok(run(worktree, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// Every file path of the commit `sha`, repo-relative (`git ls-tree -r
/// --name-only`).
pub fn tree_files(repo_root: &Path, sha: &str) -> Result<Vec<String>, GitError> {
    let out = run(repo_root, &["ls-tree", "-r", "--name-only", sha])?;
    Ok(non_empty_lines(&out))
}

/// The content of `path` at `rev` when it is a file there.
pub fn blob_at(cwd: &Path, rev: &str, path: &str) -> Option<Vec<u8>> {
    run_bytes(cwd, &["cat-file", "blob", &format!("{rev}:{path}")]).ok()
}

/// `<short sha> <subject>` of each commit in `from..to`, newest first.
pub fn log_subjects(repo: &Path, from: &str, to: &str) -> Result<Vec<String>, GitError> {
    let range = format!("{from}..{to}");
    let out = run(repo, &["log", "--format=%h %s", &range])?;
    Ok(out.lines().map(str::to_string).collect())
}

/// Whether `sha` is reachable from `rev`; `None` when git cannot tell (the
/// branch or the commit is gone).
pub fn is_ancestor(repo: &Path, sha: &str, rev: &str) -> Option<bool> {
    match exit_code(repo, &["merge-base", "--is-ancestor", sha, rev]) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Committer time of `sha`, ms since the epoch.
pub fn commit_time_ms(repo: &Path, sha: &str) -> Option<i64> {
    let out = run(repo, &["show", "-s", "--format=%ct", sha]).ok()?;
    out.trim().parse::<i64>().ok().map(|s| s * 1000)
}

/// A commit on a branch after a landing, with the files it changed.
pub struct LaterCommit {
    pub sha: String,
    pub ts_ms: i64,
    /// Carries orchd's `Task-Id:` trailer.
    pub by_orchd: bool,
    pub files: Vec<String>,
}

/// The commits in `sha..rev`, oldest first.
pub fn commits_after(repo: &Path, sha: &str, rev: &str) -> Result<Vec<LaterCommit>, GitError> {
    let range = format!("{sha}..{rev}");
    let out = run(
        repo,
        &[
            "log",
            "--reverse",
            "--name-only",
            "--format=%x1e%H%x1f%ct%x1f%B%x1f",
            &range,
        ],
    )?;
    Ok(out
        .split('\x1e')
        .filter(|c| !c.trim().is_empty())
        .filter_map(|chunk| {
            let mut parts = chunk.splitn(4, '\x1f');
            let (sha, ts, body, files) =
                (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            Some(LaterCommit {
                sha: sha.trim().to_string(),
                ts_ms: ts.trim().parse::<i64>().ok()? * 1000,
                by_orchd: body.lines().any(|l| l.starts_with("Task-Id:")),
                files: files
                    .lines()
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect())
}
