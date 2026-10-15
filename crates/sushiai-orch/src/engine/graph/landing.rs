use crate::task::{Attempt, FailureKind};
use std::path::Path;

pub(in crate::engine) enum Landing {
    /// Committed and fast-forwarded onto the parent's branch (or the parent
    /// already held the work).
    Landed,
    /// Back to the agent as an ordinary failure; it retries on the new base.
    Failed {
        kind: FailureKind,
        detail: String,
    },
    Cancelled,
    /// The parent is gone, or landing on the base was refused: commit like a
    /// task without one.
    NoParent,
    /// Landing was refused because the base is the default branch and landing
    /// there is not allowed: like `NoParent`, and the work may get a pull
    /// request.
    DefaultBranchRefused,
    /// The base branch's checkout has uncommitted changes: the task waits
    /// `landing` and is retried.
    Waiting,
    /// The carry changed a file the review saw: the attempt's verify and
    /// review run again on the carried tree, with no new implement attempt.
    Rereview,
}

/// Start of the decision line of a landing that sent the attempt back to
/// verify and the review.
pub(in crate::engine) const REREVIEW_LINE: &str = "Land: the carry";

/// How often one task goes back to verify and the review after a landing
/// carry before the carried tree lands on the checks alone.
pub(in crate::engine) const MAX_REREVIEWS: u32 = 3;

/// The files the review saw that the carry onto `new_base` rewrote: files
/// the task changed, in the reviewed tree or the carried one, that differ
/// between the two. Empty without a review.
pub(in crate::engine) async fn carry_changed_reviewed(
    worktree: &Path,
    attempt: &Attempt,
    new_base: &str,
) -> Vec<String> {
    let (Some(tree), Some(old_base)) = (attempt.review_tree.clone(), attempt.review_base.clone())
    else {
        return Vec::new();
    };
    let (wt, new_base) = (worktree.to_path_buf(), new_base.to_string());
    tokio::task::spawn_blocking(move || {
        let now = crate::git::changes::worktree_tree(&wt)?;
        crate::git::changes::own_changes_between(&wt, &tree, &old_base, &now, &new_base)
    })
    .await
    .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())))
    .unwrap_or_default()
}
