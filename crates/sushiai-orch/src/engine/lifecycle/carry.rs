use crate::delivery::git::carry::Rebase;
use crate::engine::graph::relay::conflict_detail;
use crate::engine::protected::carry::adopt_carried_tests;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::git::GitError;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use std::path::Path;

pub(in crate::engine) mod attempt;
mod checkout;
mod contained;

use checkout::CarriedWork;

/// Decision line for a carry that stopped on conflicts.
pub(in crate::engine) fn rebase_conflict_note(
    base_ref: &str,
    new_sha: &str,
    files: &[String],
    rewritten: bool,
) -> String {
    if rewritten {
        format!(
            "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {} with conflicts in {}",
            short_sha(new_sha),
            files.join(", ")
        )
    } else {
        format!(
            "Rebase: carried the work onto {base_ref} at {} with conflicts in {}",
            short_sha(new_sha),
            files.join(", ")
        )
    }
}

pub(in crate::engine) enum BaseCarry {
    Stale,
    Stopped,
    Ready,
    Conflict(String),
    /// Carrying went wrong in a way that may pass on another try.
    Failed(String),
    /// Carrying was left out on purpose; trying again changes nothing.
    Skipped(String),
}

/// Carries committed and uncommitted task work onto the current base head;
/// the planner's test files are written again after.
pub(in crate::engine) async fn carry_onto_newest_base(
    app: &App,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> BaseCarry {
    let Some(base_ref) = task.base_ref.clone() else {
        let reason = "Rebase: cannot carry the work without a base branch".to_string();
        task.record_decision(reason.clone());
        return BaseCarry::Skipped(reason);
    };
    let before = task.clone();
    let (wt, mut snapshot, r) = (worktree.to_path_buf(), task.clone(), base_ref.clone());
    let carried = tokio::task::spawn_blocking(move || checkout::carry(&mut snapshot, &wt, &r))
        .await
        .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
    let result = record_outcome(task, &base_ref, carried);
    if crate::task::checks::snapshot::CheckSnapshot::capture(&before).matches(task) {
        return result;
    }
    publish_result(app, task, &before, cancel, result)
}

pub(in crate::engine) async fn carry_relay_onto_parent(
    app: &App,
    task: &mut Task,
    parent: &Task,
    chain_base: &str,
    cancel: &CancelToken,
) -> BaseCarry {
    let before = task.clone();
    let (mut snapshot, wt, from, branch) = (
        task.clone(),
        Path::new(&task.worktree).to_path_buf(),
        chain_base.to_string(),
        parent.branch.clone(),
    );
    let carried = tokio::task::spawn_blocking(move || {
        let carried = checkout::carry_relay(&mut snapshot, &wt, &from, &branch);
        (snapshot, carried)
    })
    .await;
    let (mut next, carried) = carried
        .unwrap_or_else(|error| (before.clone(), Err(crate::git::GitError(error.to_string()))));
    let result = record_outcome(&mut next, &parent.branch, carried);
    if matches!(&result, BaseCarry::Ready | BaseCarry::Conflict(_)) {
        next.record_relay_base(Some(next.base_sha.clone()));
    }
    *task = next;
    publish_result(app, task, &before, cancel, result)
}

fn publish_result(
    app: &App,
    task: &mut Task,
    before: &Task,
    cancel: &CancelToken,
    result: BaseCarry,
) -> BaseCarry {
    use crate::engine::delivery_gate::{publish_carry, DeliveryGate};
    match publish_carry(app, task, before, cancel) {
        Ok(DeliveryGate::Ready) => result,
        Ok(DeliveryGate::Recheck) => {
            if let Ok(Some(fresh)) = app.store.load_task(&task.id) {
                *task = fresh;
            }
            BaseCarry::Stale
        }
        Ok(DeliveryGate::Stopped) => BaseCarry::Stopped,
        Err(error) => BaseCarry::Failed(error),
    }
}

fn record_outcome(
    task: &mut Task,
    base_ref: &str,
    carried: Result<CarriedWork, GitError>,
) -> BaseCarry {
    match carried {
        Ok(CarriedWork {
            rebase: Rebase::Unchanged,
            ..
        }) => BaseCarry::Ready,
        Ok(CarriedWork {
            rebase: Rebase::Moved { new_sha, rewritten },
            already_contained,
            tests,
        }) => {
            adopt_carried_tests(
                task,
                tests,
                &Rebase::Moved {
                    new_sha: new_sha.clone(),
                    rewritten,
                },
            );
            task.record_decision(if already_contained {
                format!(
                    "Rebase: the task branch already contains {base_ref} at {}; moved base_sha to that head",
                    short_sha(&new_sha)
                )
            } else if rewritten {
                format!(
                    "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {}",
                    short_sha(&new_sha)
                )
            } else {
                format!(
                    "Rebase: carried the work onto {base_ref} at {}",
                    short_sha(&new_sha)
                )
            });
            crate::task::transitions::move_base(task, new_sha);
            BaseCarry::Ready
        }
        Ok(CarriedWork {
            rebase:
                Rebase::Conflicts {
                    new_sha,
                    files,
                    rewritten,
                },
            tests,
            ..
        }) => {
            crate::task::checks::edits::adopt_planner_tests(task, tests);
            task.record_decision(rebase_conflict_note(base_ref, &new_sha, &files, rewritten));
            let detail = conflict_detail(base_ref, &new_sha, &files);
            task.record_carry_conflicts(files);
            crate::task::transitions::move_base(task, new_sha);
            BaseCarry::Conflict(detail)
        }
        Ok(CarriedWork {
            rebase: Rebase::Skipped { reason },
            ..
        }) => {
            let detail = format!("Rebase: not carried onto {base_ref}: {reason}");
            task.record_decision(detail.clone());
            BaseCarry::Skipped(detail)
        }
        Err(e) => {
            let detail = format!("Rebase: carrying onto {base_ref} failed: {e}");
            task.record_decision(detail.clone());
            BaseCarry::Failed(detail)
        }
    }
}
