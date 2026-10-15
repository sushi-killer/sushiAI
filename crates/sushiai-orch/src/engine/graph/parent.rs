use crate::clock::now_ms;
use crate::engine::graph::landing::Landing;
use crate::engine::graph::parent_checks::checked_for_delivery;
use crate::engine::graph::relay::remove_chain_branches;
use crate::engine::ladder;
use crate::engine::land::run::land_on_base;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::harness::cancel::CancelToken;
use crate::task::checks::edits::heal_grep_dashes;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;

/// How a parent's loop ended.
pub(in crate::engine) enum ParentEnd {
    /// Nothing more for this loop to do: the parent is done, parked, waiting
    /// for its children or landing, or stopped.
    Finished,
    /// Every subtask landed but finishing the parent failed. The loop goes on
    /// as an ordinary implement attempt on the parent's own worktree, with
    /// `failure` in its brief. `budget` caps the attempts (`None`: the usual
    /// `maxAttempts`).
    Attempt {
        failure: String,
        budget: Option<u32>,
    },
}

/// A parent whose checks passed (or that has none) is done; with
/// `variant.land` it first lands on its base branch as one commit titled
/// with its title, exactly like a single task. A landing that fails goes to
/// an agent attempt.
pub(in crate::engine) async fn land_parent(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> ParentEnd {
    let task_id = task.id.clone();
    let worktree = PathBuf::from(&task.worktree);
    let mut landed = false;
    // A subtask no review read already landed on this branch: the parent
    // is unreviewed too, is not landed by itself, and the owner gets a PR.
    ladder::inherit_unreviewed(app, task);
    if !task.unreviewed && task.variant().land && task.parent.is_none() {
        let run_dir = app.store.task_dir(&task_id).join("runs").join("parent");
        let _ = std::fs::create_dir_all(&run_dir);
        match land_on_base(
            app,
            task,
            &crate::engine::graph::carry::CarryTree::new(&worktree, &run_dir, None, cancel),
        )
        .await
        {
            Landing::Landed => {
                crate::delivery::git::carry::delete_wip_ref(&worktree, &task_id);
                landed = true;
            }
            Landing::Waiting => {
                task.resume_phase(TaskStatus::Landing, crate::clock::now_ms());
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                return ParentEnd::Finished;
            }
            Landing::Failed { detail, .. } => {
                task.resume_phase(TaskStatus::Running, crate::clock::now_ms());
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                return ParentEnd::Attempt {
                    failure: detail,
                    budget: None,
                };
            }
            Landing::Cancelled => {
                app.save_task_logged(task);
                mark_stopped_if_not_already(app, &task_id).await;
                return ParentEnd::Finished;
            }
            // A parent has no review to run again.
            Landing::Rereview => {}
            // Refused: done on its own branch, which gets a pull request.
            Landing::NoParent | Landing::DefaultBranchRefused => {}
        }
    }
    if !landed {
        // A clean carry turned the landed child commits back into a diff.
        // Keep that work on the parent's branch before removing its worktree.
        let committed = commit_carried_parent(task).await;
        if let Err(e) = committed {
            let failure = format!("Orchestrator: could not commit the parent's carried work: {e}");
            task.record_decision(failure.clone());
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            return ParentEnd::Attempt {
                failure,
                budget: Some(1),
            };
        }
        crate::delivery::git::carry::delete_wip_ref(&worktree, &task_id);
    }
    crate::task::transitions::complete(
        task,
        None,
        crate::task::transitions::Completion::Done,
        now_ms(),
    );
    crate::engine::delivery::pr::pending::mark_pr_pending(app, task);
    task.touch(now_ms());
    app.write_report(task).await;
    app.release_worktree(
        task,
        if landed {
            "landed"
        } else {
            "every subtask landed"
        },
    )
    .await;
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    app.notify_task_unreviewed(task);
    wake_orchestrator_for_done(app, task);
    crate::engine::delivery::pr::pending::spawn_pr_on_done(app, task);
    ParentEnd::Finished
}

use crate::engine::graph::parent_setup::{
    children_ready, dependencies_ready, prepare_parent, start_children,
};
use crate::engine::run_control::TaskSignals;
pub(in crate::engine) async fn run_parent(
    app: &Arc<App>,
    task_id: &str,
    signals: &TaskSignals,
) -> ParentEnd {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };
    let cancel = &signals.cancel;
    if let Err(error) =
        apply_pending_amendment(app, &mut task, &signals.pending_amend, cancel).await
    {
        eprintln!("orch: applying amendment to {} failed: {error}", task.id);
        return ParentEnd::Finished;
    }
    if heal_grep_dashes(&mut task) {
        app.verify_cache.lock().unwrap().remove(task_id);
        task.touch(now_ms());
        app.save_task_logged(&task);
        app.broadcast_task(&task);
    }
    let all = app.repo_tasks(&task.repo);
    if !dependencies_ready(app, &mut task, &all) {
        return ParentEnd::Finished;
    }
    if task.status == TaskStatus::Landing {
        let permit = tokio::select! { _=cancel.cancelled()=>None,p=app.slots.clone().acquire_owned()=>p.ok() };
        if permit.is_none() {
            mark_stopped_if_not_already(app, task_id).await;
            return ParentEnd::Finished;
        }
        return land_parent(app, &mut task, cancel).await;
    }
    if !prepare_parent(app, &mut task, &all, signals).await {
        return ParentEnd::Finished;
    }
    let all = start_children(app, &mut task, all, cancel);
    if !children_ready(app, &mut task, &all) {
        return ParentEnd::Finished;
    }
    let permit =
        tokio::select! { _=cancel.cancelled()=>None,p=app.slots.clone().acquire_owned()=>p.ok() };
    if permit.is_none() {
        mark_stopped_if_not_already(app, task_id).await;
        return ParentEnd::Finished;
    }
    task.resume_phase(TaskStatus::Running, now_ms());
    task.touch(now_ms());
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    let mut task = match checked_for_delivery(app, &mut task, signals).await {
        Ok(task) => task,
        Err(end) => return end,
    };
    task.record_decision(format!(
        "Orchestrator: every subtask landed on {}",
        task.branch
    ));
    remove_chain_branches(app, &mut task).await;
    land_parent(app, &mut task, cancel).await
}

async fn commit_carried_parent(task: &Task) -> Result<(), crate::git::GitError> {
    let (wt, title, tid) = (
        PathBuf::from(&task.worktree),
        task.title.clone(),
        task.id.clone(),
    );
    tokio::task::spawn_blocking(move || {
        if crate::git::changes::status_short(&wt)?.trim().is_empty() {
            Ok(())
        } else {
            crate::delivery::git::commit::commit(&wt, &title, &tid, 0)
        }
    })
    .await
    .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())))
}
