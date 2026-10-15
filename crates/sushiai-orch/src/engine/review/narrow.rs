use crate::task::Attempt;
use crate::task::CriterionRuling;
use crate::task::ReviewResult;
use crate::task::Stage;
use crate::task::Task;
use crate::task::Verdict;
use std::path::Path;

/// The earlier implement attempt a retry review builds on: the latest one
/// before `attempt_n` that has a review. `None` for a first attempt.
pub(super) fn latest_reviewed_before(task: &Task, attempt_n: u32) -> Option<&Attempt> {
    task.attempts
        .iter()
        .rev()
        .find(|a| a.stage == Stage::Implement && a.n < attempt_n && a.review.is_some())
}

/// The attempt a narrow review of attempt `attempt_n` is built on: the latest
/// reviewed earlier attempt, when its review failed and recorded a tree.
pub(in crate::engine) fn narrow_basis(task: &Task, attempt_n: u32) -> Option<&Attempt> {
    latest_reviewed_before(task, attempt_n).filter(|a| {
        a.review_tree.is_some()
            && a.review
                .as_ref()
                .is_some_and(|r| r.verdict == Verdict::Fail)
    })
}

/// What a narrow review is given.
pub(super) struct NarrowInputs {
    /// The criteria (index, text) the basis review did not rule met.
    pub(super) open_criteria: Vec<(usize, String)>,
    /// Criteria the basis review ruled met, and that attempt's number.
    pub(super) settled: Vec<(usize, String)>,
    settled_from: u32,
    pub(super) delta: String,
}

/// The narrow review's inputs from the basis attempt's failed review, or
/// `None` when the delta cannot be computed (a git error): the review is then
/// a full one.
pub(super) async fn narrow_inputs(
    task: &Task,
    basis: &Attempt,
    now_tree: &str,
    worktree: &Path,
    base_sha: &str,
) -> Option<NarrowInputs> {
    let review = basis.review.as_ref()?;
    let old_tree = basis.review_tree.clone()?;
    let old_base = basis.review_base.clone();
    let (wt, now, base) = (
        worktree.to_path_buf(),
        now_tree.to_string(),
        base_sha.to_string(),
    );
    let delta = tokio::task::spawn_blocking(move || -> Option<String> {
        let files = match &old_base {
            Some(old_base) => {
                crate::git::changes::own_changes_between(&wt, &old_tree, old_base, &now, &base)
            }
            None => crate::git::changes::diff_trees_name_only(&wt, &old_tree, &now),
        }
        .ok()?;
        if files.is_empty() {
            return Some("(no file changed since the last review)".to_string());
        }
        crate::git::changes::diff_trees(&wt, &old_tree, &now, &files, 60_000).ok()
    })
    .await
    .ok()??;
    let mut settled = Vec::new();
    let mut open_criteria = Vec::new();
    for (i, criterion) in task.criteria.iter().enumerate() {
        let met = review
            .criteria
            .iter()
            .any(|r| r.met == Some(true) && r.index == Some(i));
        if met {
            settled.push((i, criterion.clone()));
        } else {
            open_criteria.push((i, criterion.clone()));
        }
    }
    Some(NarrowInputs {
        open_criteria,
        settled,
        settled_from: basis.n,
        delta,
    })
}

/// Copies the settled rulings into a narrow review's result, so the criteria
/// results and the verdict rule still see them as met. A ruling the reviewer
/// gave now wins.
pub(super) fn carry_settled(narrow: &NarrowInputs, result: &mut ReviewResult) {
    for (i, criterion) in &narrow.settled {
        let ruled_now = result.criteria.iter().any(|r| r.index == Some(*i));
        if !ruled_now {
            result.criteria.push(CriterionRuling {
                criterion: criterion.clone(),
                index: Some(*i),
                met: Some(true),
                evidence: Some(format!(
                    "settled in the attempt {} review",
                    narrow.settled_from
                )),
            });
        }
    }
}
