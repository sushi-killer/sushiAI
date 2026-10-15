use crate::delivery::git::carry::wip_ref;
use crate::git::bootstrap::bootstrap_worktree;
use crate::git::branches::{branch_exists, checkout_of};
use crate::git::command::{quiet_git, run};
use crate::git::worktree::exclude_worktree_root;
use crate::git::GitError;
use std::path::Path;

/// Removes a worktree and its branch that nothing has used yet (best
/// effort: an undone step leaves at most an unused branch or directory).
pub fn discard_worktree(repo_root: &Path, path: &Path, branch: &str) {
    crate::procs::stop_processes_under(path);
    if let Some(p) = path.to_str() {
        let _ = run(repo_root, &["worktree", "remove", "--force", p]);
    }
    let _ = run(repo_root, &["branch", "-D", branch]);
}

/// Removes a task's worktree directory (its branch stays) and prunes the
/// repo's worktree list.
pub fn remove_task_worktree(repo_root: &Path, path: &Path) {
    crate::procs::stop_processes_under(path);
    if let Some(p) = path.to_str() {
        let _ = run(repo_root, &["worktree", "remove", "--force", p]);
    }
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = run(repo_root, &["worktree", "prune"]);
}

/// Recreates a removed worktree at `path` from `branch`, bootstraps it, and
/// puts back the work kept at [`wip_ref`] when that was saved on top of the
/// branch's current head. `true` when saved work was restored.
pub fn restore_worktree(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    task_id: &str,
) -> Result<bool, GitError> {
    let _ = run(repo_root, &["worktree", "prune"]);
    if path.join(".git").exists() {
        return Ok(false);
    }
    if !branch_exists(repo_root, branch) {
        return Err(GitError(format!(
            "branch {branch} no longer exists (a subtask branch is removed once it landed on its parent's branch)"
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GitError(e.to_string()))?;
        let _ = exclude_worktree_root(repo_root, parent);
    }
    let p = path
        .to_str()
        .ok_or_else(|| GitError("non-utf8 path".into()))?;
    run(repo_root, &["worktree", "add", p, branch])?;
    let _ = bootstrap_worktree(repo_root, path);
    let wip_name = wip_ref(task_id);
    let Ok(wip) = run(
        path,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("{wip_name}^{{commit}}"),
        ],
    ) else {
        return Ok(false);
    };
    let wip = wip.trim().to_string();
    let parent = run(path, &["rev-parse", &format!("{wip}^")]).map(|s| s.trim().to_string());
    let head = run(path, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    if parent.is_err() || parent.ok() != head.ok() {
        return Ok(false);
    }
    let quiet = quiet_git();
    if run(
        path,
        &[&quiet[..], &["cherry-pick", "--no-commit", &wip]].concat(),
    )
    .is_err()
    {
        let _ = run(path, &["cherry-pick", "--quit"]);
        let _ = run(path, &["reset", "-q", "--hard"]);
        return Ok(false);
    }
    let _ = run(path, &["cherry-pick", "--quit"]);
    run(path, &["reset", "-q"])?;
    let _ = run(path, &["update-ref", "-d", &wip_name]);
    Ok(true)
}

/// Removes a task's branch and saved work (best effort).
pub fn delete_branch(repo_root: &Path, branch: &str, task_id: &str) {
    let _ = run(repo_root, &["update-ref", "-d", &wip_ref(task_id)]);
    let _ = run(repo_root, &["branch", "-D", branch]);
}

/// Deletes, locally, each `(task id, branch)` that is fully contained in
/// `into` (`merge-base --is-ancestor`) together with its saved-work ref. The
/// ref is deleted with the sha that was checked, so a branch that moved
/// meanwhile stays. `into` itself and a branch checked out in any worktree
/// stay. Returns the branches removed.
pub fn delete_merged_branches(
    repo_root: &Path,
    into: &str,
    candidates: &[(String, String)],
) -> Vec<String> {
    let mut removed = Vec::new();
    for (task_id, branch) in candidates {
        let full = format!("refs/heads/{branch}");
        if branch == into || checkout_of(repo_root, branch).is_some() {
            continue;
        }
        let Ok(sha) = run(repo_root, &["rev-parse", "--verify", "-q", &full]) else {
            continue;
        };
        let sha = sha.trim();
        if run(repo_root, &["merge-base", "--is-ancestor", sha, into]).is_err() {
            continue;
        }
        if run(repo_root, &["update-ref", "-d", &full, sha]).is_ok() {
            let _ = run(repo_root, &["update-ref", "-d", &wip_ref(task_id)]);
            removed.push(branch.clone());
        }
    }
    removed
}
