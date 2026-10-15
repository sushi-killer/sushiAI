use crate::engine::fresh_base;
use crate::engine::graph::relay::conflict_detail;
use crate::engine::protected::adopt_carried_tests;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use std::path::Path;

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
    let carried = tokio::task::spawn_blocking(move || {
        let from = snapshot.base_sha.clone();
        let tip = snapshot.fresh_base.as_ref().and_then(|f| f.tip.clone());
        let new_sha = crate::git::history::resolve_commit(
            &wt,
            &fresh_base::carry_rev(&wt, &r, tip.as_deref()),
        )?;
        if new_sha == from {
            return Ok((
                crate::delivery::git::carry::Rebase::Unchanged,
                false,
                snapshot.planner_tests.clone(),
            ));
        }
        // Preserve a merge the task already made; resetting it would carry
        // the base's changes a second time along with the task's own work.
        if crate::git::history::is_ancestor(&wt, &new_sha, "HEAD") == Some(true) {
            return Ok((
                crate::delivery::git::carry::Rebase::Moved {
                    new_sha,
                    rewritten: false,
                },
                true,
                snapshot.planner_tests.clone(),
            ));
        }
        // Work committed by an attempt goes back to uncommitted.
        if crate::git::history::head_sha(&wt)? != from {
            crate::delivery::git::carry::uncommit_to(&wt, &from)?;
        }
        let carried = fresh_base::carry_work(&mut snapshot, &wt, &r)?;
        Ok((carried, false, snapshot.planner_tests.clone()))
    })
    .await
    .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
    let result = match carried {
        Ok((crate::delivery::git::carry::Rebase::Unchanged, _, _)) => BaseCarry::Ready,
        Ok((
            crate::delivery::git::carry::Rebase::Moved { new_sha, rewritten },
            already_contained,
            tests,
        )) => {
            adopt_carried_tests(
                task,
                tests,
                &crate::delivery::git::carry::Rebase::Moved {
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
        Ok((
            crate::delivery::git::carry::Rebase::Conflicts {
                new_sha,
                files,
                rewritten,
            },
            _,
            tests,
        )) => {
            crate::task::checks::edits::adopt_planner_tests(task, tests);
            task.record_decision(rebase_conflict_note(&base_ref, &new_sha, &files, rewritten));
            let detail = conflict_detail(&base_ref, &new_sha, &files, &task.id);
            task.record_carry_conflicts(files);
            crate::task::transitions::move_base(task, new_sha);
            BaseCarry::Conflict(detail)
        }
        Ok((crate::delivery::git::carry::Rebase::Skipped { reason }, _, _)) => {
            let detail = format!("Rebase: not carried onto {base_ref}: {reason}");
            task.record_decision(detail.clone());
            BaseCarry::Skipped(detail)
        }
        Err(e) => {
            let detail = format!("Rebase: carrying onto {base_ref} failed: {e}");
            task.record_decision(detail.clone());
            BaseCarry::Failed(detail)
        }
    };
    if crate::task::checks::snapshot::CheckSnapshot::capture(&before).matches(task) {
        return result;
    }
    use crate::engine::delivery_gate::{publish_carry, DeliveryGate};
    match publish_carry(app, task, &before, cancel) {
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
