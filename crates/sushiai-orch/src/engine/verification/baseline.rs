use crate::clock::now_ms;
use crate::engine::state::App;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::harness::compiler_cache::CacheRun;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::types::Baseline;
use crate::task::checks::types::Check;
use crate::task::state::Task;
use crate::verify::output::classify_baseline;
use std::path::Path;
use std::sync::Arc;

/// Runs the checks that have no baseline yet in `worktree` and stores what
/// each one did on the base: `pass` (proves nothing), `fail` (gates), `env`
/// (could not run). Whatever the commands changed in the worktree is undone.
/// `false` when cancelled, with nothing stored.
pub(in crate::engine) async fn baseline_checks(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> bool {
    let ticket = crate::task::checks::baseline::BaselineTicket::capture(task);
    let todo = &ticket.todo;
    if todo.is_empty() {
        return true;
    }
    let wt = worktree.to_path_buf();
    let before = tokio::task::spawn_blocking(move || {
        crate::git::changes::status_entries(&wt).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let commands: Vec<String> = todo.iter().map(|(_, _, run)| run.clone()).collect();
    let run_dir = app.store.task_dir(task_id).join("runs/baseline");
    let sandbox = app.task_sandbox(&task.repo, worktree, &run_dir);
    let cache_run = CacheRun::for_commands(worktree, &commands).await;
    let runner =
        CheckRunner::new(app, worktree, sandbox.as_ref(), &task.base_sha, cancel).for_task(task_id);
    let mut results = Vec::new();
    let mut decisions = Vec::new();
    for ((slot, criterion, _), command) in todo.iter().zip(&commands) {
        let label = slot.map_or_else(
            || crate::task::checks::commands::held_out_label(*criterion),
            |_| command.clone(),
        );
        let (result, lines) = runner.run_one(command, &label, None).await;
        results.push(result);
        decisions.extend(lines);
    }
    decisions.extend(cache_run.finish("baseline checks").await);
    record_check_decisions(task, decisions);
    let wt = worktree.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || crate::git::changes::restore_status(&wt, &before))
        .await;
    if cancel.is_cancelled() {
        return false;
    }
    record_check_outcomes(app, task, &results);
    record_baselines(task, &ticket, &results)
}

/// Classifies IO results before attaching them to the captured check plan.
pub(in crate::engine) fn record_baselines(
    task: &mut Task,
    ticket: &crate::task::checks::baseline::BaselineTicket,
    results: &[VerifyOutcome],
) -> bool {
    let observations: Option<Vec<_>> = results
        .iter()
        .map(|result| {
            Some(crate::task::checks::baseline::BaselineObservation {
                baseline: classify_baseline(result)?,
                code: result.code,
            })
        })
        .collect();
    observations.is_some_and(|observations| {
        crate::task::checks::baseline::record(task, ticket, &observations, now_ms())
    })
}

/// Some check (or the held-out one) has not run on the base yet.
pub(in crate::engine) fn needs_baseline(task: &Task) -> bool {
    task.checks.iter().any(|c| c.baseline.is_none())
        || (task.variant().held_out && task.held_out.as_ref().is_some_and(|h| h.baseline.is_none()))
}

/// The gated checks of `task`: visible ones that fail on the base, then the
/// held-out one.
pub(in crate::engine) fn gated_checks(task: &Task) -> Vec<Check> {
    task.checks
        .iter()
        .filter(|c| c.baseline == Some(Baseline::Fail))
        .cloned()
        .collect()
}

pub(in crate::engine) fn gated_held_out(task: &Task) -> Option<Check> {
    task.held_out
        .clone()
        .filter(|h| task.variant().held_out && h.baseline == Some(Baseline::Fail))
}

#[cfg(test)]
#[path = "baseline/tests.rs"]
mod tests;
