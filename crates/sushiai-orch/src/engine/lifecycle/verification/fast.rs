use super::Verification;
use crate::engine::decision::failure;
use crate::engine::delivery::pr::repair::step::{settle_step, StepOutcome};
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::engine::verification::base::answers::base_check_answered;
use crate::engine::verification::base::checks::broken_by_the_new_base;
use crate::engine::verification::base::jobs::BaseCheckJob;
use crate::engine::verification::base::wait::{BaseBreak, BaseFailure};
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::context::{
    record_check_decisions, record_check_outcomes, CheckContext,
};
use crate::engine::verification::healing::preflight::settle_base_preflight;
use crate::engine::verification::runner::CheckRunner;
use crate::engine::verification::scoping::scope_task_checks;
use crate::harness::cancel::CancelToken;
use crate::task::attempts::{record_verify, VerifyObservation};
use crate::task::{FailureKind, Task, VerifyOutcome};
use crate::verify::load::timed_out_under_load;
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) async fn verify(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    slot: &mut QuestionSlot<'_>,
) -> Result<Vec<VerifyOutcome>, Verification> {
    let cancel = slot.cancel().clone();
    let mut results = execute(app, task, run, &cancel).await;
    interrupted(app, task, run, &cancel)?;
    let mut preflight_checked = false;
    let mut waived = BTreeSet::new();
    while let Some(failed) = results
        .iter()
        .find(|result| result.code != Some(0) && !waived.contains(&result.command))
        .cloned()
    {
        if !preflight_checked && !timed_out_under_load(&failed) {
            preflight_checked = true;
            let before = task.verify.clone();
            match settle_base_preflight(app, task, &cancel).await {
                Ok(()) => {}
                Err(WaitEnd::Changed) => return Err(Verification::Changed),
                Err(WaitEnd::Stopped) => {
                    interrupt_attempt(app, task, run.idx);
                    return Err(Verification::Stopped);
                }
            }
            if task.verify != before {
                results = execute(app, task, run, &cancel).await;
                interrupted(app, task, run, &cancel)?;
                continue;
            }
        }
        let files = AttemptFiles::new(
            run.idx,
            run.attempt_n,
            run.worktree.clone(),
            run.run_dir.clone(),
        );
        match settle_step(app, task, &files, &failed, slot).await {
            StepOutcome::NotRepair => {}
            StepOutcome::Waived => {
                waived.insert(failed.command.clone());
                continue;
            }
            StepOutcome::Changed => return Err(Verification::Changed),
            StepOutcome::Retry => return Err(Verification::Retry(0)),
            StepOutcome::Waiting | StepOutcome::Stopped => return Err(Verification::Stopped),
        }
        if moved_base(app, task, run, &failed, slot).await? {
            results.retain(|result| result.command != failed.command);
            record_check_outcomes(app, task, &results);
            record_verify(task, run.idx, VerifyObservation::Replaced(results.clone()));
            continue;
        }
        return Err(Verification::Failed(failure(
            FailureKind::Verify,
            detail(&failed),
        )));
    }
    Ok(results)
}

async fn execute(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let (commands, skipped) =
        scope_task_checks(app, task, &run.worktree, &task.base_sha, &task.verify).await;
    record_check_decisions(task, skipped);
    let sandbox = app.task_sandbox(&task.repo, &run.worktree, &run.run_dir);
    let (results, lines) = run_verify_cached(
        CheckRunner::from_context(
            CheckContext::new(app, Some(&task.id)),
            &run.worktree,
            sandbox.as_ref(),
            &task.base_sha,
            cancel,
        ),
        &task.id,
        &commands,
    )
    .await;
    record_check_decisions(task, lines);
    if !cancel.is_cancelled() {
        record_check_outcomes(app, task, &results);
        record_verify(task, run.idx, VerifyObservation::Replaced(results.clone()));
    }
    results
}

async fn moved_base(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    failed: &VerifyOutcome,
    slot: &mut QuestionSlot<'_>,
) -> Result<bool, Verification> {
    if run.base_sha == task.base_sha || base_check_answered(task, &failed.command, &task.base_sha) {
        return Ok(false);
    }
    let cancel = slot.cancel().clone();
    let job = BaseCheckJob::new(
        Some(&task.id),
        &task.repo,
        &task.base_sha,
        &run.run_dir.join("verify-base"),
        false,
    );
    let Some(base) = broken_by_the_new_base(app, job, failed, &run.base_sha, &cancel).await else {
        return Ok(false);
    };
    match BaseFailure::new(run.idx, failed, &base)
        .wait(app, task, slot)
        .await
    {
        BaseBreak::Dropped => Ok(true),
        BaseBreak::Retry => Err(Verification::Retry(1)),
        BaseBreak::Changed => Err(Verification::Changed),
        BaseBreak::Stopped => Err(Verification::Stopped),
    }
}

fn interrupted(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    cancel: &CancelToken,
) -> Result<(), Verification> {
    if cancel.is_cancelled() {
        interrupt_attempt(app, task, run.idx);
        Err(Verification::Stopped)
    } else {
        Ok(())
    }
}

pub(super) fn detail(failed: &VerifyOutcome) -> String {
    format!(
        "{} exited {}.\n{}",
        failed.command,
        failed
            .code
            .map_or_else(|| "null".into(), |code| code.to_string()),
        failed.tail
    )
}
