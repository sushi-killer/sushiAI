use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::answer_policy::remove_check_command;
use crate::engine::decision::record_failure;
use crate::engine::lifecycle::base_wait::carry_after_base_moves;
use crate::engine::lifecycle::base_wait::BaseRetry;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::reply::Reply;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::engine::verification::base::answers::record_owner_base_check;
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::engine::verification::base::questions::pre_existing_question;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::FailureKind;
use crate::task::attempt::VerifyOutcome;
use crate::task::lifecycle::is_option;
use crate::task::lifecycle::PRE_EXISTING_RETRY;
use crate::task::state::Task;
use crate::task::state::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// What the owner decided about a `verify` command the new base broke.
pub(in crate::engine) enum BaseBreak {
    /// The loop was cancelled or stopped while waiting.
    Stopped,
    /// Try again on the newest base: the attempt is not counted.
    Retry,
    /// The check is gone from `verify`; the attempt goes on without it.
    Dropped,
}

/// A `verify` command fails on the new base alone, so the attempt is not to
/// blame: the failure is recorded with a signature of its own and does not
/// count against `attempt_budget`, and the task waits for the owner (the
/// same question a final check that fails on the base gets). The detail
/// tells the next attempt not to fix what is out of its scope.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn wait_on_broken_base(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    failed: &VerifyOutcome,
    base_run: &VerifyOutcome,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> BaseBreak {
    let command = failed.command.clone();
    let base_sha = task.base_sha.clone();
    task.record_decision(format!(
        "Orchestrator: {command} fails on the base {} too",
        short_sha(&base_sha)
    ));
    let code = failed
        .code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "null".to_string());
    record_failure(
        task,
        idx,
        FailureKind::Verify,
        format!(
            "{command} exited {code}, and it fails on the base {} without this task's work: the base broke it, not this attempt. Do not fix it as part of this task.\n{}",
            short_sha(&base_sha),
            failed.tail
        ),
    );
    crate::task::attempts::record_failure_signature(task, idx, format!("base:{command}"));
    *attempt_budget += 1;
    let assumed = task.assumptions.len();
    let already_retried = base_retry_requested(task, std::slice::from_ref(&command), &base_sha);
    let answer = if already_retried {
        PRE_EXISTING_RETRY.to_string()
    } else {
        task.ask(
            pre_existing_question(std::slice::from_ref(base_run), &base_sha),
            now_ms(),
        );
        task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        task.touch(now_ms());
        let Some(answer) = wait_for_answer(app, task, pending_answer, cancel, permit).await else {
            return BaseBreak::Stopped;
        };
        answer
    };
    let policy_answered = task.assumptions.len() > assumed;
    if !already_retried {
        record_owner_base_check(
            task,
            std::slice::from_ref(&command),
            &base_sha,
            &answer,
            policy_answered,
        );
    }
    if is_option(&answer, PRE_EXISTING_DROP) {
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        match remove_check_command(&settings, task, &command, origin) {
            Ok(()) => {
                task.record_decision(format!("Orchestrator: dropped check {command}"));
                app.verify_cache.lock().unwrap().remove(task_id);
                *attempt_budget -= 1;
                crate::task::transitions::reopen(task, idx, now_ms());
                task.resume_phase(TaskStatus::Running, crate::clock::now_ms());
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                return BaseBreak::Dropped;
            }
            // Refused: the check stays and the attempt stays failed. The
            // answer counts as a retry on the newest base.
            Err(why) => task.record_decision(format!("Orchestrator: check {command} stays: {why}")),
        }
    }
    let wt = PathBuf::from(&task.worktree);
    match carry_after_base_moves(app, task, &wt, pending_answer, cancel, permit).await {
        BaseRetry::Ready => {}
        BaseRetry::Conflict(detail) => record_failure(task, idx, FailureKind::Conflict, detail),
        BaseRetry::Stopped => return BaseBreak::Stopped,
    }
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    BaseBreak::Retry
}
