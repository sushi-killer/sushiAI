use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::baseline::judge_against_base;
use crate::engine::baseline::BaseVerdict;
use crate::engine::decision::fail_and_continue;
use crate::engine::decision::record_failure;
use crate::engine::decision::LoopSignal;
use crate::engine::graph::base::apply_base_check_answer;
use crate::engine::lifecycle::base_wait::carry_after_base_moves;
use crate::engine::lifecycle::base_wait::BaseRetry;
use crate::engine::lifecycle::completion::admission::back_to_review;
use crate::engine::lifecycle::completion::delivery::finish_attempt;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::final_checks::run_final_checks;
use crate::engine::lifecycle::final_checks::FinalChecks;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::reply::Reply;
use crate::engine::rate_limit::counted_attempts;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::engine::verification::base::answers::command_named_by_task;
use crate::engine::verification::base::answers::is_kept;
use crate::engine::verification::base::answers::keep_check;
use crate::engine::verification::base::answers::keep_planned_checks;
use crate::engine::verification::base::answers::record_owner_base_check;
use crate::engine::verification::base::answers::KEPT_BY_TASK_TARGET;
use crate::engine::verification::base::checks::failing_on_base;
use crate::engine::verification::base::checks::note_flaky_base_checks;
use crate::engine::verification::base::questions::pre_existing_question;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::healing::preflight::settle_base_preflight;
use crate::engine::verification::scoping::deferred_line;
use crate::engine::verification::scoping::split_deferred;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::attempts::{self as observations, VerifyObservation};
use crate::task::FailureKind;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::VerifyOutcome;
use crate::task::PRE_EXISTING_RETRY;
use crate::verify::load::timed_out_under_load;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// What a failing final check tells the next attempt.
fn final_failure_detail(failed: &VerifyOutcome) -> String {
    format!(
        "Final check {} exited {}.\n{}",
        failed.command,
        failed
            .code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".to_string()),
        failed.tail
    )
}

/// Settles retained final results, or runs the checks for an accepted attempt.
/// A failing final is a Verify failure or a pre-existing-check question.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn finish_after_review(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    settings: &Settings,
    finished: Option<FinalChecks>,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Tail {
    // Settle the base preflight before interpreting final results or landing.
    if !settle_base_preflight(app, task, cancel).await {
        interrupt_attempt(app, task, idx);
        *permit = None;
        app.finish_task_loop(task_id);
        return Tail::Return;
    }
    // The commands the parent runs anyway are left out of this run; the
    // stored `finalVerify` is not changed.
    let (to_run, deferred) = split_deferred(app, settings, task);
    record_check_decisions(task, deferred.iter().map(|c| deferred_line(c)).collect());
    if !to_run.is_empty() {
        // Not through the verify cache: it keys on the diff alone and
        // would hand back the fast checks' results.
        let FinalChecks {
            results: final_results,
            skipped,
        } = match finished {
            Some(finished) => finished,
            None => {
                let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
                run_final_checks(
                    app,
                    task,
                    worktree,
                    run_dir,
                    &task.base_sha,
                    &to_run,
                    sandbox.as_ref(),
                    cancel,
                    None,
                )
                .await
            }
        };
        record_check_decisions(task, skipped);
        record_check_outcomes(app, task, &final_results);
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, idx);
            *permit = None;
            app.finish_task_loop(task_id);
            return Tail::Return;
        }
        observations::record_verify(
            task,
            idx,
            VerifyObservation::Appended(final_results.clone()),
        );
        let failed_finals: Vec<VerifyOutcome> = final_results
            .iter()
            .filter(|v| v.code != Some(0))
            .cloned()
            .collect();
        // Every failing final goes through the base lookup (cache, else the
        // in-flight job, else a job started now), never a second run.
        let base_dir = run_dir.join("final-base");
        let mut on_base: Vec<(VerifyOutcome, VerifyOutcome)> = Vec::new();
        let mut ordinary: Vec<VerifyOutcome> = Vec::new();
        for failed in failed_finals {
            if timed_out_under_load(&failed) {
                ordinary.push(failed);
                continue;
            }
            match failing_on_base(app, task, &failed, &base_dir, cancel).await {
                Some(base_run) => on_base.push((failed, base_run)),
                None => ordinary.push(failed),
            }
        }
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, idx);
            *permit = None;
            app.finish_task_loop(task_id);
            return Tail::Return;
        }
        note_flaky_base_checks(app, task, &to_run);
        // A base failure the plan explains (the files it names are created
        // by the plan) or the owner kept is an ordinary failure.
        let names: Vec<String> = on_base.iter().map(|(f, _)| f.command.clone()).collect();
        let kept = keep_planned_checks(app, task, &names);
        let mut pre_existing: Vec<VerifyOutcome> = Vec::new();
        for (failed, base_run) in on_base {
            // A check the task is meant to turn green is never waived.
            if !kept.contains(&failed.command)
                && !is_kept(task, &failed.command)
                && command_named_by_task(task, &failed.command)
            {
                keep_check(task, &failed.command, KEPT_BY_TASK_TARGET);
            }
            if kept.contains(&failed.command) || is_kept(task, &failed.command) {
                ordinary.push(failed);
                continue;
            }
            // The base is red too: only a failure the base lacks is the task's.
            match judge_against_base(app, task, &failed, &base_run, worktree, run_dir, cancel).await
            {
                BaseVerdict::Waived => {}
                BaseVerdict::NewFailures(own) => ordinary.push(*own),
                BaseVerdict::Unclear => pre_existing.push(base_run),
            }
        }
        if !kept.is_empty() {
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        let ordinary_detail = ordinary
            .iter()
            .map(final_failure_detail)
            .collect::<Vec<_>>()
            .join("\n\n");
        if pre_existing.is_empty() {
            if !ordinary.is_empty() {
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    ordinary_detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
        } else {
            // The failure is older than this task: no retry can fix it. The
            // owner decides, once for all of them, and no attempt is spent.
            let mut detail: Vec<String> = pre_existing
                .iter()
                .map(|b| {
                    let failed = final_results.iter().find(|v| v.command == b.command);
                    failed.map(final_failure_detail).unwrap_or_default()
                })
                .collect();
            if !ordinary_detail.is_empty() {
                detail.push(ordinary_detail.clone());
            }
            record_failure(task, idx, FailureKind::Verify, detail.join("\n\n"));
            let question = pre_existing_question(&pre_existing, &task.base_sha);
            let assumed = task.assumptions.len();
            let already_retried = base_retry_requested(task, &question.commands, &task.base_sha);
            let answer = if already_retried {
                PRE_EXISTING_RETRY.to_string()
            } else {
                task.ask(question.clone(), task.updated_at);
                task.resume_phase(TaskStatus::Waiting, task.updated_at);
                task.touch(now_ms());
                let Some(answer) = wait_for_answer(app, task, pending_answer, cancel, permit).await
                else {
                    *permit = None;
                    app.finish_task_loop(task_id);
                    return Tail::Return;
                };
                answer
            };
            let policy_answered = task.assumptions.len() > assumed;
            if !already_retried {
                let base_sha = task.base_sha.clone();
                record_owner_base_check(
                    task,
                    &question.commands,
                    &base_sha,
                    &answer,
                    policy_answered,
                );
            }
            let settings = app.settings.read().unwrap().clone();
            let origin = origin_of_last_answer(task);
            // Every removal goes through `remove_check_command`; a refused
            // command stays and the answer counts as a retry for it.
            if apply_base_check_answer(&settings, task, &question, &answer, origin) {
                app.verify_cache.lock().unwrap().remove(task_id);
            }
            let kept_all =
                !question.commands.is_empty() && question.commands.iter().all(|c| is_kept(task, c));
            if kept_all {
                let mut details: Vec<String> = final_results
                    .iter()
                    .filter(|result| question.commands.contains(&result.command))
                    .map(final_failure_detail)
                    .collect();
                if !ordinary_detail.is_empty() {
                    details.push(ordinary_detail.clone());
                }
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    details.join("\n\n"),
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
            let stays = question
                .commands
                .iter()
                .any(|c| task.final_verify.contains(c));
            if stays {
                // Retry (any other answer): one more attempt on the newest
                // base, even when the attempt budget is spent.
                match carry_after_base_moves(app, task, worktree, pending_answer, cancel, permit)
                    .await
                {
                    BaseRetry::Ready => {}
                    BaseRetry::Conflict(detail) => {
                        record_failure(task, idx, FailureKind::Conflict, detail);
                    }
                    BaseRetry::Stopped => {
                        interrupt_attempt(app, task, idx);
                        *permit = None;
                        app.finish_task_loop(task_id);
                        return Tail::Return;
                    }
                }
                *attempt_budget = (*attempt_budget).max(counted_attempts(task) + 1);
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                return Tail::Continue;
            }
            // Every listed check is gone: the attempt is not failed by them.
            crate::task::transitions::reopen(task, idx, now_ms());
            task.resume_phase(TaskStatus::Running, task.updated_at);
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            // The failing check cost the attempt its review: it is judged
            // again, never landed without a verdict.
            if ordinary.is_empty()
                && task.attempts[idx].review.is_none()
                && !settings.review.is_empty()
            {
                task.record_decision(format!(
                    "Orchestrator: attempt {attempt_n} reviewed again because its review was discarded by a check that failed on the base"
                ));
                return back_to_review(app, task, idx, permit);
            }
            if !ordinary.is_empty() {
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    ordinary_detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
        }
    }
    finish_attempt(
        app,
        task_id,
        task,
        idx,
        attempt_n,
        worktree,
        run_dir,
        attempt_budget,
        pending_answer,
        cancel,
        permit,
    )
    .await
}
