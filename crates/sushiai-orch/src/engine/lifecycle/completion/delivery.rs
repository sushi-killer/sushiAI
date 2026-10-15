use crate::clock::now_ms;
use crate::engine::decision::fail_and_continue;
use crate::engine::decision::record_failure;
use crate::engine::decision::LoopSignal;
use crate::engine::graph::landing::Landing;
use crate::engine::ladder;
use crate::engine::land::land_on_base;
use crate::engine::lifecycle::completion::admission::back_to_review;
use crate::engine::lifecycle::completion::admission::delivery_admission;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::questions::reply::Reply;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::reports::refresh_diff_stat;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_eval_check;
use crate::harness::cancel::CancelToken;
use crate::task::EvalCheck;
use crate::task::FailureKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// What follows a passed attempt: the task lands on its parent or its base
/// branch when it has one to land on, else it commits on its own branch.
/// Ends the loop itself (`Tail::Return`) or hands back to it.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn finish_attempt(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Tail {
    if let Some(tail) = delivery_admission(app, task, idx, cancel, permit) {
        return tail;
    }

    // Top-level work no review read is never landed by itself: it waits on
    // its branch and a pull request marked unreviewed goes to the owner. A
    // subtask lands on its parent's branch, which is not the default branch;
    // the parent then carries the flag.
    ladder::inherit_unreviewed(app, task);
    let landing = if task.unreviewed && task.parent.is_none() {
        None
    } else if let Some(parent_id) = task.parent.clone() {
        Some(
            crate::engine::graph::carry::CarryRun::new(app, task, Some(idx), run_dir, cancel)
                .in_worktree(worktree)
                .land_on_parent(&parent_id)
                .await,
        )
    } else if task.variant().land {
        Some(land_on_base(app, task, Some(idx), attempt_n, worktree, run_dir, cancel).await)
    } else {
        None
    };
    if let Some(landing) = landing {
        match landing {
            Landing::Waiting => {
                crate::task::transitions::complete(
                    task,
                    Some(idx),
                    crate::task::transitions::Completion::WaitingForLanding,
                    now_ms(),
                );
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            Landing::Landed => {
                crate::delivery::git::carry::delete_wip_ref(worktree, task_id);
                crate::task::transitions::complete(
                    task,
                    Some(idx),
                    crate::task::transitions::Completion::Done,
                    now_ms(),
                );
                task.touch(now_ms());
                app.write_report(task).await;
                app.release_worktree(task, "landed").await;
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            Landing::Failed { kind, detail } => {
                match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    kind,
                    detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        return Tail::Continue;
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        return Tail::Return;
                    }
                }
            }
            // The loop follows the finished run again: its verify and review
            // judge the carried tree, with no new implement attempt.
            Landing::Rereview => return back_to_review(app, task, idx, permit),
            Landing::Cancelled => {
                crate::task::attempts::end(
                    task,
                    idx,
                    crate::task::attempts::AttemptEnd::Interrupted,
                    now_ms(),
                );
                app.apply_cancellation(task);
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            // Refused: done on its own branch, which gets a pull request.
            Landing::NoParent | Landing::DefaultBranchRefused => {}
        }
    }

    let wt4 = worktree.to_path_buf();
    let title = task.title.clone();
    let tid = task_id.to_string();
    let commit_res = tokio::task::spawn_blocking(move || {
        crate::delivery::git::commit::commit(&wt4, &title, &tid, attempt_n)
    })
    .await;
    match commit_res {
        Ok(Ok(())) => {
            crate::delivery::git::carry::delete_wip_ref(worktree, task_id);
            refresh_diff_stat(task).await;
            crate::task::transitions::complete(
                task,
                Some(idx),
                crate::task::transitions::Completion::Done,
                now_ms(),
            );
            if let Some(cmd) = task.eval_check_cmd.clone() {
                let sha = crate::git::history::head_sha(worktree).unwrap_or_default();
                let check = run_eval_check(
                    crate::engine::verification::runner::CheckRunner::from_context(
                        CheckContext::new(app, Some(task_id)),
                        &(run_dir).join("eval-check"),
                        app.task_sandbox(&task.repo, &run_dir.join("eval-check"), run_dir)
                            .as_ref(),
                        &sha,
                        cancel,
                    ),
                    Path::new(&task.repo),
                    &cmd,
                )
                .await;
                record_check_outcomes(app, task, std::slice::from_ref(&check));
                task.record_eval_check(EvalCheck {
                    code: check.code,
                    tail: check.tail,
                });
            }
        }
        Ok(Err(e)) => {
            record_failure(task, idx, FailureKind::Error, e.to_string());
            task.mark_failed();
        }
        Err(e) => {
            record_failure(
                task,
                idx,
                FailureKind::Error,
                format!("commit task panicked: {e}"),
            );
            task.mark_failed();
        }
    }
    if task.status == TaskStatus::Done {
        crate::engine::delivery::pr::pending::mark_pr_pending(app, task);
        task.touch(now_ms());
        app.write_report(task).await;
        app.release_worktree(task, "committed on its branch").await;
    }
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    wake_orchestrator_for_done(app, task);
    crate::engine::delivery::pr::pending::spawn_pr_on_done(app, task);
    *permit = None;
    app.finish_task_loop(task_id);
    Tail::Return
}
