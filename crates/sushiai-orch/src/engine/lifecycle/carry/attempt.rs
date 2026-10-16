use super::rebase_conflict_note;
use crate::delivery::git::carry::Rebase;
use crate::engine::decision::failure;
use crate::engine::fresh_base;
use crate::engine::graph::relay::conflict_detail;
use crate::engine::lifecycle::changes::record_changed_files;
use crate::engine::lifecycle::completion::admission::{carry_admission, delivery_admission};
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::protected::carry::adopt_carried_tests;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::task::{Failure, FailureKind, Task};
use std::sync::Arc;

pub(in crate::engine) enum CarryEnd {
    Tail(Tail),
    Failed(Failure),
}

pub(in crate::engine) async fn carry(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    changed: &mut Vec<String>,
    slot: &mut QuestionSlot<'_>,
) -> Result<(), CarryEnd> {
    let Some(base_ref) = task.base_ref.clone() else {
        return Ok(());
    };
    let before = task.clone();
    let carried = execute(task, run, &base_ref).await;
    if let Ok(Rebase::Moved { new_sha, .. } | Rebase::Conflicts { new_sha, .. }) = &carried {
        crate::task::transitions::move_base(task, new_sha.clone());
        if let Some(tail) = carry_admission(app, task, &before, slot) {
            return Err(CarryEnd::Tail(tail));
        }
    }
    match carried {
        Ok(Rebase::Unchanged) => {}
        Ok(Rebase::Moved { new_sha, rewritten }) => {
            task.record_decision(if rewritten {
                format!(
                    "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {}",
                    short_sha(&new_sha)
                )
            } else {
                format!("Rebase: carried the work onto {base_ref} at {}", short_sha(&new_sha))
            });
            *changed = record_changed_files(task, run).await;
            if changed.is_empty() {
                if let Some(tail) = delivery_admission(app, task, run.idx, slot) {
                    return Err(CarryEnd::Tail(tail));
                }
                super::contained::complete(app, task, run.idx, &base_ref, slot).await;
                return Err(CarryEnd::Tail(Tail::Return));
            }
        }
        Ok(Rebase::Skipped { reason }) => {
            record_once(
                task,
                format!("Rebase: not carried onto {base_ref} ({reason})"),
            );
        }
        Ok(Rebase::Conflicts {
            new_sha,
            files,
            rewritten,
        }) => {
            if rewritten {
                task.record_decision(rebase_conflict_note(&base_ref, &new_sha, &files, rewritten));
            }
            task.record_carry_conflicts(files.clone());
            return Err(CarryEnd::Failed(failure(
                FailureKind::Conflict,
                conflict_detail(&base_ref, &new_sha, &files),
            )));
        }
        Err(error) => {
            record_once(task, format!(
                "Rebase: could not carry the work onto {base_ref} ({error}); verifying on the old base"
            ));
        }
    }
    Ok(())
}

async fn execute(
    task: &mut Task,
    run: &RunReceipt,
    base_ref: &str,
) -> Result<Rebase, crate::git::GitError> {
    let (worktree, mut snapshot, base) = (run.worktree.clone(), task.clone(), base_ref.to_string());
    let (carried, tests) = tokio::task::spawn_blocking(move || {
        let carried = fresh_base::carry_work(&mut snapshot, &worktree, &base)?;
        Ok((carried, snapshot.planner_tests.clone()))
    })
    .await
    .unwrap_or_else(|error| Err(crate::git::GitError(error.to_string())))?;
    adopt_carried_tests(task, tests, &carried);
    Ok(carried)
}

fn record_once(task: &mut Task, note: String) {
    if !task.decisions.contains(&note) {
        task.record_decision(note);
    }
}
