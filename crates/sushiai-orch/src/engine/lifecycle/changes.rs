use crate::engine::review::narrow_basis;
use crate::engine::time_budget;
use crate::harness;
use crate::task::attempts::{self as observations, AttemptReport};
use crate::task::FailureKind;
use crate::task::ReviewScope;
use crate::task::Stage;
use crate::task::Task;
use std::path::Path;

pub(in crate::engine) async fn capture_run_interruption(
    task: &mut Task,
    idx: usize,
    outcome: &harness::RunOutcome,
    worktree: &Path,
    base_sha: &str,
    budget_mins: f64,
) -> Option<(FailureKind, String)> {
    let (kind, detail) = if outcome.time_budget {
        (
            FailureKind::TimeBudget,
            outcome.error.clone().unwrap_or_default(),
        )
    } else if outcome.stalled || outcome.over_budget {
        (
            if outcome.over_budget {
                FailureKind::Budget
            } else {
                FailureKind::Stall
            },
            outcome.error.clone().unwrap_or_default(),
        )
    } else if let Some(detail) = &outcome.looped {
        (FailureKind::Loop, detail.clone())
    } else {
        return None;
    };
    let wt = worktree.to_path_buf();
    let changed = if outcome.time_budget {
        let base = base_sha.to_owned();
        tokio::task::spawn_blocking(move || {
            crate::git::changes::changed_files(&wt, &base).unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    } else {
        let own = task.own_diff();
        tokio::task::spawn_blocking(move || own.changed_files(&wt).unwrap_or_default())
            .await
            .unwrap_or_default()
    };
    if outcome.time_budget {
        let report =
            time_budget::report_after_budget(budget_mins, outcome.final_text.as_deref(), &changed);
        let declines = task.attempts[idx].declines.clone();
        observations::record_report(
            task,
            idx,
            AttemptReport {
                summary: report.summary,
                handoff: Some(report.handoff),
                declines,
            },
        );
    }
    observations::record_changed_files(task, idx, changed);
    record_diff_stat(task, idx, worktree).await;
    Some((kind, detail))
}

/// The files the task changed since the last full review that no open finding
/// of the review the narrow one built on names, by `path:line`. Empty when
/// nothing is unnamed or it cannot be told (no full review with a tree, a git
/// error).
pub(in crate::engine) async fn unnamed_changes_since_full_review(
    task: &Task,
    attempt_n: u32,
    now_tree: Option<&str>,
    worktree: &Path,
    base_sha: &str,
) -> Vec<String> {
    let Some(now_tree) = now_tree else {
        return Vec::new();
    };
    let Some(open) = narrow_basis(task, attempt_n).and_then(|a| a.review.as_ref()) else {
        return Vec::new();
    };
    let Some((full_tree, full_base)) = task
        .attempts
        .iter()
        .rev()
        .filter(|a| a.stage == Stage::Implement && a.n < attempt_n)
        .find(|a| a.review_scope == Some(ReviewScope::Full) && a.review_tree.is_some())
        .and_then(|a| Some((a.review_tree.clone()?, a.review_base.clone())))
    else {
        return Vec::new();
    };
    let named = crate::verify::review::reply::finding_paths(&open.findings);
    let (wt, now, base) = (
        worktree.to_path_buf(),
        now_tree.to_string(),
        base_sha.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        let files = match &full_base {
            Some(old_base) => {
                crate::git::changes::own_changes_between(&wt, &full_tree, old_base, &now, &base)
            }
            None => crate::git::changes::diff_trees_name_only(&wt, &full_tree, &now),
        }
        .unwrap_or_default();
        files
            .into_iter()
            .filter(|f| !named.contains(f))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}

/// Records the size of the agent's own work on the attempt and the task.
pub(in crate::engine) async fn record_diff_stat(task: &mut Task, idx: usize, worktree: &Path) {
    let (wt, own) = (worktree.to_path_buf(), task.own_diff());
    let stat = tokio::task::spawn_blocking(move || own.stat(&wt).ok())
        .await
        .unwrap_or(None);
    if let Some(stat) = stat {
        observations::record_diff_stat(task, idx, stat);
    }
}
