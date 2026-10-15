use crate::git::command::{quiet_git, run};
use crate::git::history::{head_sha, resolve_commit};
use crate::git::GitError;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum Rebase {
    Unchanged,
    /// `rewritten`: the recorded base is not an ancestor of the new head.
    Moved {
        new_sha: String,
        rewritten: bool,
    },
    /// The worktree now sits on `new_sha` with conflicts in `files`.
    Conflicts {
        new_sha: String,
        files: Vec<String>,
        rewritten: bool,
    },
    /// Left as it was, on purpose: `reason` says why.
    Skipped {
        reason: String,
    },
}

pub fn wip_ref(task_id: &str) -> String {
    format!("refs/orchd/wip/{task_id}")
}

/// Moves a worktree nothing has touched onto the current head of its base
/// branch before planning: `Some(new_sha)` when it moved, `None` when the
/// base did not move or the worktree has commits or tracked changes of its own.
pub fn reset_untouched_to_base(
    worktree: &Path,
    base_ref: &str,
    base_sha: &str,
) -> Result<Option<String>, GitError> {
    let new_sha = run(
        worktree,
        &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")],
    )?
    .trim()
    .to_string();
    if new_sha == base_sha {
        return Ok(None);
    }
    if run(worktree, &["rev-parse", "HEAD"])?.trim() != base_sha {
        return Ok(None);
    }
    if !run(worktree, &["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty()
    {
        return Ok(None);
    }
    run(worktree, &["reset", "-q", "--hard", &new_sha])?;
    Ok(Some(new_sha))
}

/// Carries the agent's uncommitted work from `base_sha` onto wherever
/// `base_ref` points now. The work is first saved as a commit kept at
/// [`wip_ref`], the branch moves to the new base, and the work is applied
/// back uncommitted with the recorded base as merge base, so the same holds
/// when the base was rewritten (amended) rather than moved ahead; the result
/// says which. Conflicts are left for the agent. Anything that would lose
/// work is skipped instead: commits of the agent's own on the branch, or
/// ignored local files the new base starts tracking.
/// A failure midway restores the worktree exactly as it was.
pub fn carry_onto_moved_base(
    worktree: &Path,
    base_ref: &str,
    base_sha: &str,
    task_id: &str,
) -> Result<Rebase, GitError> {
    let skipped = |reason: &str| {
        Ok(Rebase::Skipped {
            reason: reason.to_string(),
        })
    };
    let new_sha = run(
        worktree,
        &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")],
    )?
    .trim()
    .to_string();
    if new_sha == base_sha {
        return Ok(Rebase::Unchanged);
    }
    let head = run(worktree, &["rev-parse", "HEAD"])?.trim().to_string();
    if head != base_sha {
        // Someone merged the new base into the branch: nothing to carry.
        if run(worktree, &["merge-base", "--is-ancestor", &new_sha, "HEAD"]).is_ok() {
            return Ok(Rebase::Moved {
                new_sha,
                rewritten: false,
            });
        }
        return skipped("the task branch has commits of its own");
    }
    let rewritten = run(
        worktree,
        &["merge-base", "--is-ancestor", base_sha, &new_sha],
    )
    .is_err();
    let tracked_there: std::collections::HashSet<String> =
        run(worktree, &["ls-tree", "-r", "--name-only", &new_sha])?
            .lines()
            .map(str::to_string)
            .collect();
    let clobbered: Vec<String> = run(
        worktree,
        &["ls-files", "-o", "-i", "--exclude-standard", "--directory"],
    )?
    .lines()
    .filter(|f| tracked_there.contains(*f))
    .map(str::to_string)
    .collect();
    if !clobbered.is_empty() {
        return skipped(&format!(
            "the new base tracks ignored local files: {}",
            clobbered.join(", ")
        ));
    }
    let dirty = !run(worktree, &["status", "--porcelain"])?.trim().is_empty();
    if !dirty {
        run(worktree, &["reset", "-q", "--hard", &new_sha])?;
        return Ok(Rebase::Moved { new_sha, rewritten });
    }
    let quiet = quiet_git();
    run(worktree, &[&quiet[..], &["add", "-A"]].concat())?;
    run(
        worktree,
        &[
            &quiet[..],
            &["commit", "-q", "--no-verify", "-m", "orchd wip"],
        ]
        .concat(),
    )?;
    let wip = run(worktree, &["rev-parse", "HEAD"])?.trim().to_string();
    run(worktree, &["update-ref", &wip_ref(task_id), &wip])?;
    apply_saved_work(worktree, base_sha, &new_sha, &wip, rewritten)
}

fn apply_saved_work(
    worktree: &Path,
    base_sha: &str,
    new_sha: &str,
    wip: &str,
    rewritten: bool,
) -> Result<Rebase, GitError> {
    let quiet = quiet_git();
    let apply = || -> Result<Rebase, GitError> {
        run(worktree, &["reset", "-q", "--hard", new_sha])?;
        let picked = run(
            worktree,
            &[&quiet[..], &["cherry-pick", "--no-commit", wip]].concat(),
        );
        let files: Vec<String> = run(worktree, &["diff", "--name-only", "--diff-filter=U"])?
            .lines()
            .map(str::to_string)
            .collect();
        if let (Err(e), true) = (picked, files.is_empty()) {
            return Err(e);
        }
        let _ = run(worktree, &["cherry-pick", "--quit"]);
        run(worktree, &["reset", "-q"])?;
        Ok(if files.is_empty() {
            Rebase::Moved {
                new_sha: new_sha.to_string(),
                rewritten,
            }
        } else {
            Rebase::Conflicts {
                new_sha: new_sha.to_string(),
                files,
                rewritten,
            }
        })
    };
    apply().inspect_err(|_| {
        // Back to the agent's uncommitted work on the old base.
        let _ = run(worktree, &["cherry-pick", "--quit"]);
        let _ = run(worktree, &["reset", "-q", "--hard", wip]);
        let _ = run(worktree, &["reset", "-q", "--soft", base_sha]);
        let _ = run(worktree, &["reset", "-q"]);
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum BaseMerge {
    Unchanged,
    Merged { head: String },
    Conflicts { files: Vec<String> },
}

/// Paths introduced by the incoming side since the branches diverged.
pub fn incoming_paths(worktree: &Path, base_ref: &str) -> Result<Vec<String>, GitError> {
    Ok(run(
        worktree,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &format!("HEAD...{base_ref}"),
        ],
    )?
    .split('\0')
    .filter(|p| !p.is_empty())
    .map(str::to_string)
    .collect())
}

/// Merges a base into a clean graph branch. A conflict is always aborted;
/// conflict markers belong only in a part's carry, never in the parent.
pub fn merge_base(worktree: &Path, base_ref: &str) -> Result<BaseMerge, GitError> {
    let base = resolve_commit(worktree, base_ref)?;
    if run(worktree, &["merge-base", "--is-ancestor", &base, "HEAD"]).is_ok() {
        return Ok(BaseMerge::Unchanged);
    }
    if !run(worktree, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(GitError("the graph branch has uncommitted changes".into()));
    }
    let merged = run(
        worktree,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=orchd",
            "-c",
            "user.email=orchd@localhost",
            "merge",
            "--no-edit",
            "--no-verify",
            &base,
        ],
    );
    if let Err(error) = merged {
        let files: Vec<String> = run(worktree, &["diff", "--name-only", "--diff-filter=U", "-z"])?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        if run(worktree, &["rev-parse", "--verify", "MERGE_HEAD"]).is_ok() {
            run(
                worktree,
                &["-c", "core.hooksPath=/dev/null", "merge", "--abort"],
            )?;
        }
        return if files.is_empty() {
            Err(error)
        } else {
            Ok(BaseMerge::Conflicts { files })
        };
    }
    Ok(BaseMerge::Merged {
        head: head_sha(worktree)?,
    })
}

/// Turns every commit on the worktree's branch since `base_sha` back into
/// uncommitted work on `base_sha` (`git reset <base_sha>`); the files stay
/// as they are.
pub fn uncommit_to(worktree: &Path, base_sha: &str) -> Result<(), GitError> {
    run(worktree, &["reset", "-q", base_sha]).map(|_| ())
}

/// Drops the saved work of a task that no longer needs it.
pub fn delete_wip_ref(worktree: &Path, task_id: &str) {
    let _ = run(worktree, &["update-ref", "-d", &wip_ref(task_id)]);
}
