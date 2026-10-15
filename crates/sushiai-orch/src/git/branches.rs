use crate::git::command::run;
use crate::git::GitError;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// lowercase, [a-z0-9-], collapsed, max 40 chars, no leading/trailing '-'.
pub fn slugify(title: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > max_len {
        let mut end = max_len;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("task");
    }
    out
}

pub fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Default branch name `task/<slug>`, disambiguated with `-2`, `-3`, ... if
/// it already exists.
pub fn unique_branch_name(repo_root: &Path, title: &str) -> String {
    let slug = slugify(title, 40);
    let base = format!("task/{slug}");
    if !branch_exists(repo_root, &base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !branch_exists(repo_root, &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// The local branch `rev` names (for `HEAD`, the checked-out branch), or
/// `None` for a sha or a detached checkout.
pub fn branch_of(repo_root: &Path, rev: &str) -> Option<String> {
    if rev == "HEAD" {
        return run(repo_root, &["symbolic-ref", "--short", "-q", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    run(
        repo_root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{rev}"),
        ],
    )
    .ok()
    .map(|_| rev.to_string())
}

/// The repo's default branch: what `origin/HEAD` points at, else `main`,
/// else `master`, whichever exists locally.
pub fn default_branch(repo: &Path) -> Option<String> {
    if let Ok(out) = run(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = out.trim().strip_prefix("origin/") {
            return Some(name.to_string());
        }
    }
    ["main", "master"]
        .into_iter()
        .find_map(|b| branch_of(repo, b))
}

/// The worktree (usually the main checkout) that has `branch` checked out.
pub fn checkout_of(repo: &Path, branch: &str) -> Option<PathBuf> {
    let out = run(repo, &["worktree", "list", "--porcelain"]).ok()?;
    let want = format!("branch refs/heads/{branch}");
    out.split("\n\n").find_map(|block| {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
        block
            .lines()
            .any(|l| l == want)
            .then(|| PathBuf::from(path))
    })
}

/// Whether a checkout has uncommitted changes to tracked files.
pub fn has_uncommitted_changes(checkout: &Path) -> Result<bool, GitError> {
    Ok(
        !run(checkout, &["status", "--porcelain", "--untracked-files=no"])?
            .trim()
            .is_empty(),
    )
}

/// Moves `refs/heads/<branch>` to `sha` only if it still is `expected`.
pub fn update_branch(repo: &Path, branch: &str, sha: &str, expected: &str) -> Result<(), GitError> {
    run(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), sha, expected],
    )
    .map(|_| ())
}

/// `git merge --ff-only <sha>` in `worktree`: moves its checked-out branch
/// (and its files) to `sha`, or fails without touching anything when that
/// is not a fast-forward.
pub fn fast_forward(worktree: &Path, sha: &str) -> Result<(), GitError> {
    run(
        worktree,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "merge",
            "-q",
            "--ff-only",
            sha,
        ],
    )
    .map(|_| ())
}

/// What [`move_branch`] did.
#[derive(Debug)]
pub enum BranchMove {
    /// The branch is at the commit; `checkout` is where it is checked out, or
    /// the repo root when it is checked out nowhere.
    Done { checkout: PathBuf },
    /// The branch's checkout has uncommitted changes; nothing was touched.
    Dirty { checkout: PathBuf },
    /// The branch is no longer an ancestor of the commit.
    Diverged,
}

/// Fast-forwards `branch` to `sha`: `git merge --ff-only` in the worktree
/// that has it checked out (left alone when it has uncommitted changes), else
/// `git update-ref` expecting the branch's current value. Never pushes.
pub fn move_branch(repo: &Path, branch: &str, sha: &str) -> Result<BranchMove, GitError> {
    let refname = format!("refs/heads/{branch}");
    let head = run(repo, &["rev-parse", "--verify", &refname])?
        .trim()
        .to_string();
    if run(repo, &["merge-base", "--is-ancestor", &head, sha]).is_err() {
        return Ok(BranchMove::Diverged);
    }
    let Some(checkout) = checkout_of(repo, branch) else {
        update_branch(repo, branch, sha, &head)?;
        return Ok(BranchMove::Done {
            checkout: repo.to_path_buf(),
        });
    };
    if has_uncommitted_changes(&checkout)? {
        return Ok(BranchMove::Dirty { checkout });
    }
    match fast_forward(&checkout, sha) {
        Ok(()) => Ok(BranchMove::Done { checkout }),
        // Nothing moved: an untracked file in the way, not a moved branch.
        Err(_)
            if run(repo, &["rev-parse", "--verify", &refname])
                .ok()
                .map(|s| s.trim().to_string())
                .as_deref()
                == Some(head.as_str()) =>
        {
            Ok(BranchMove::Dirty { checkout })
        }
        Err(e) => Err(e),
    }
}
