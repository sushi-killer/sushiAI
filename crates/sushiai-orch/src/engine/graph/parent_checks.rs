use crate::clock::now_ms;
use crate::engine::delivery_gate::{begin_delivery, DeliveryGate};
use crate::engine::graph::base::parent_finals_on_base;
use crate::engine::graph::base::ParentGate;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::baseline::gated_held_out;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::held_out::run_held_out;
use crate::engine::verification::runner::run_verify_commands;
use crate::engine::verification::scoping::scope_task_checks;
use crate::harness::cancel::CancelToken;
use crate::prompts::text::tail_chars;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::graph::parent_cost;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

use crate::engine::graph::parent::ParentEnd;
use crate::engine::run_control::TaskSignals;
use crate::task::VerifyOutcome;
pub(in crate::engine::graph) async fn ordinary_checks(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
) -> Result<(Task, Vec<VerifyOutcome>, Vec<String>), ParentEnd> {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let cancel = &signals.cancel;
    let pending_answer = &signals.pending_answer;
    let pending_amend = &signals.pending_amend;
    loop {
        // An amendment made at any point (also while this loop waited for a
        // slot or an answer) reaches this check.
        apply_pending_amendment(app, task, pending_amend, cancel).await;
        let Ok(Some(mut task)) = app.store.load_task(task_id) else {
            return Err(ParentEnd::Finished);
        };
        let checks: Vec<String> = task
            .verify
            .iter()
            .chain(&task.final_verify)
            .cloned()
            .collect();
        let (checks, skipped) = scope_task_checks(
            app,
            &task,
            Path::new(&task.worktree),
            &task.base_sha,
            &checks,
        )
        .await;
        let sandbox = app.task_sandbox(
            &task.repo,
            Path::new(&task.worktree),
            &app.store.task_dir(task_id).join("runs/parent"),
        );
        let results = run_verify_commands(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)).with_gate("parent gate"),
                Path::new(&task.worktree),
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            &checks,
        )
        .await;
        record_check_outcomes(app, &mut task, &results);
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, task_id).await;
            return Err(ParentEnd::Finished);
        }
        match parent_finals_on_base(app, &mut task, results, pending_answer, cancel).await {
            ParentGate::Go(results) => return Ok((task, results, skipped)),
            ParentGate::Again => continue,
            ParentGate::Conflict(failure) => {
                return Err(ParentEnd::Attempt {
                    failure,
                    budget: Some(1),
                    conflict_only: true,
                });
            }
            ParentGate::End => {
                mark_stopped_if_not_already(app, task_id).await;
                return Err(ParentEnd::Finished);
            }
        }
    }
}

pub(in crate::engine::graph) async fn grounded_checks(
    app: &Arc<App>,
    task: &mut Task,
    results: &[VerifyOutcome],
    cancel: &CancelToken,
) -> Option<String> {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let sandbox = app.task_sandbox(
        &task.repo,
        Path::new(&task.worktree),
        &app.store.task_dir(task_id).join("runs/parent"),
    );
    // Once, after verify and finalVerify: the gated checks, then the held-out
    // one. The line for a failure names its kind and criterion, and never the
    // held-out command.
    let mut grounded_failure: Option<String> = None;
    if results.iter().all(|v| v.code == Some(0)) {
        let _dir = app.store.task_dir(task_id).join("runs").join("parent");
        let worktree = task.worktree.clone();
        let wt = Path::new(&worktree);
        let gated = gated_checks(task);
        let commands: Vec<String> = gated.iter().map(|c| c.run.clone()).collect();
        let gated_results = run_verify_commands(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                wt,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            &commands,
        )
        .await;
        record_check_outcomes(app, task, &gated_results);
        if let Some((check, failed)) = gated
            .iter()
            .zip(&gated_results)
            .find(|(_, v)| v.code != Some(0))
        {
            grounded_failure = Some(format!(
                "Orchestrator: every subtask landed, but the check for criterion {} failed (verify) on {}: `{}` exited {}: {}",
                check.criterion,
                task.branch,
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                tail_chars(failed.tail.trim(), 600)
            ));
        } else if let Some(held) = gated_held_out(task) {
            let (result, detail) = run_held_out(
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(app, Some(&task.id)),
                    wt,
                    sandbox.as_ref(),
                    &(task).base_sha,
                    cancel,
                ),
                task,
                &held,
            )
            .await;
            record_check_outcomes(app, task, std::slice::from_ref(&result));
            if detail.is_some() {
                grounded_failure = Some(format!(
                    "Orchestrator: every subtask landed, but the held-out check for criterion {} failed (heldout) on {}: {}",
                    held.criterion,
                    task.branch,
                    tail_chars(result.tail.trim(), 600)
                ));
            }
        }
    }
    grounded_failure
}

pub(in crate::engine::graph) fn finish_checks(
    app: &Arc<App>,
    task: Task,
    results: Vec<VerifyOutcome>,
    skipped: Vec<String>,
    grounded_failure: Option<String>,
) -> Result<Task, ParentEnd> {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let decisions = task.decisions.clone();
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return Err(ParentEnd::Finished);
    };
    record_check_decisions(&mut task, decisions);
    task.reconcile_children_cost(parent_cost(&task, &app.repo_tasks(&task.repo)));
    record_check_decisions(&mut task, skipped);
    let failure = match results.iter().find(|v| v.code != Some(0)) {
        Some(failed) => {
            let code = failed
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            if !task
                .loop_state
                .final_check_failures
                .contains(&failed.command)
            {
                task.record_final_check_failures([failed.command.clone()]);
            }
            task.record_decision(format!(
                "Orchestrator: every subtask landed, but `{}` exited {code} on {}: {}",
                failed.command,
                task.branch,
                tail_chars(failed.tail.trim(), 600)
            ));
            Some(format!(
                "`{}` exited {code} on {}:\n{}",
                failed.command,
                task.branch,
                failed.tail.trim()
            ))
        }
        None => grounded_failure.inspect(|line| task.record_decision(line.clone())),
    };
    if let Some(failure) = failure {
        // A check that fails after the subtasks landed gets one agent
        // attempt on the parent's branch, not the end of the graph.
        task.touch(now_ms());
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        return Err(ParentEnd::Attempt {
            failure,
            budget: Some(1),
            conflict_only: false,
        });
    }
    Ok(task)
}

pub(in crate::engine::graph) async fn checked_for_delivery(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
) -> Result<Task, ParentEnd> {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let cancel = &signals.cancel;
    loop {
        let (mut checked, results, skipped) = match ordinary_checks(app, task, signals).await {
            Ok(checked) => checked,
            Err(end) => return Err(end),
        };
        let expected = CheckSnapshot::capture(&checked);
        let grounded_failure = grounded_checks(app, &mut checked, &results, cancel).await;
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, task_id).await;
            return Err(ParentEnd::Finished);
        }
        apply_pending_amendment(app, &mut checked, &signals.pending_amend, cancel).await;
        if !expected.matches(&checked) {
            *task = checked;
            continue;
        }
        let mut ready = match finish_checks(app, checked, results, skipped, grounded_failure) {
            Ok(task) => task,
            Err(end) => return Err(end),
        };
        match begin_delivery(app, &mut ready, cancel, &expected) {
            Ok(DeliveryGate::Ready) => return Ok(ready),
            Ok(DeliveryGate::Stopped) => return Err(ParentEnd::Finished),
            Ok(DeliveryGate::Recheck) => {
                let Ok(Some(fresh)) = app.store.load_task(task_id) else {
                    return Err(ParentEnd::Finished);
                };
                *task = fresh;
                apply_pending_amendment(app, task, &signals.pending_amend, cancel).await;
            }
            Err(error) => {
                eprintln!("orch: admitting delivery for parent {task_id} failed: {error}");
                return Err(ParentEnd::Finished);
            }
        }
    }
}
