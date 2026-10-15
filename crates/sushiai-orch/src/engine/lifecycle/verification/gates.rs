use super::{fast::detail, Verification};
use crate::engine::decision::failure;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::state::App;
use crate::engine::verification::baseline::{gated_checks, gated_held_out};
use crate::engine::verification::context::{record_check_outcomes, CheckContext};
use crate::engine::verification::held_out::run_held_out;
use crate::engine::verification::runner::{run_verify_commands, CheckRunner};
use crate::harness::cancel::CancelToken;
use crate::task::attempts::{record_verify, VerifyObservation};
use crate::task::checks::results::refresh_criteria_results;
use crate::task::{FailureKind, Task, VerifyOutcome};
use std::sync::Arc;

pub(super) async fn verify(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    cancel: &CancelToken,
) -> Result<Vec<VerifyOutcome>, Verification> {
    let mut results = Vec::new();
    let commands: Vec<String> = gated_checks(task)
        .iter()
        .map(|check| check.run.clone())
        .collect();
    if !commands.is_empty() {
        let sandbox = app.task_sandbox(&task.repo, &run.worktree, &run.run_dir);
        results = run_verify_commands(
            CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                &run.worktree,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            &commands,
        )
        .await;
        record_check_outcomes(app, task, &results);
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, run.idx);
            return Err(Verification::Stopped);
        }
        record_verify(task, run.idx, VerifyObservation::Appended(results.clone()));
        if let Some(failed) = results.iter().find(|result| result.code != Some(0)) {
            refresh_criteria_results(task, run.idx);
            return Err(Verification::Failed(failure(
                FailureKind::Verify,
                detail(failed),
            )));
        }
    }
    if let Some(held) = gated_held_out(task) {
        let sandbox = app.task_sandbox(&task.repo, &run.worktree, &run.run_dir);
        let (result, detail) = run_held_out(
            CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                &run.worktree,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            task,
            &held,
        )
        .await;
        record_check_outcomes(app, task, std::slice::from_ref(&result));
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, run.idx);
            return Err(Verification::Stopped);
        }
        record_verify(
            task,
            run.idx,
            VerifyObservation::Appended(vec![result.clone()]),
        );
        results.push(result);
        if let Some(detail) = detail {
            refresh_criteria_results(task, run.idx);
            return Err(Verification::Failed(failure(FailureKind::Heldout, detail)));
        }
    }
    Ok(results)
}
