use crate::clock::now_ms;
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::task::ReviewResult;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

/// Review runs one implement attempt may have: the first and one more,
/// whatever asks for it (no verdict, a judge drop, a healed criterion, the
/// owner's `retry`).
pub(super) const MAX_REVIEW_RUNS: u32 = 2;

/// The decision line for a review loop that ran out of runs.
pub(super) fn review_stopped_note(attempt_n: u32) -> String {
    format!("Orchestrator: attempt {attempt_n} review stopped after {MAX_REVIEW_RUNS} runs")
}

/// Reviews of a finding after the first that make it a repeat to handle.
const HANDLE_AT_REPEATS: u32 = 2;

/// What the loop does about the findings that repeated for the second time:
/// a failing scenario becomes a criterion and a check. Every other one stays
/// open and blocking: the implementer may decline it with evidence, and the
/// next review rules on that (`apply_declines`). They no longer count as
/// repeated, so the next step is another attempt, not an owner question.
pub(super) async fn handle_repeated_findings(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    r: &mut ReviewResult,
    worktree: &Path,
    cancel: &CancelToken,
) {
    let own = r.severities.len().min(r.findings.len());
    let mut changed = false;
    for i in 0..own {
        let finding = r.findings[i].clone();
        let Some(id) = r.details.get(i).and_then(|d| d.id.clone()) else {
            continue;
        };
        let Some(pos) = task.findings.iter().position(|f| f.id == id) else {
            continue;
        };
        if task.findings[pos].handled {
            r.repeated.retain(|t| *t != finding);
            continue;
        }
        if task.findings[pos].repeats < HANDLE_AT_REPEATS {
            continue;
        }
        task.mark_finding_handled(pos);
        changed = true;
        r.repeated.retain(|t| *t != finding);
        let run = r
            .details
            .get(i)
            .and_then(|d| d.scenario.as_ref())
            .map(|s| s.run.clone())
            .or_else(|| task.findings[pos].scenarios.last().cloned())
            .filter(|run| !run.trim().is_empty());
        let Some(run) = run else {
            task.record_decision(format!(
                "Orchestrator: review finding {id} repeated twice with no scenario; it stays open"
            ));
            continue;
        };
        let dir = app
            .store
            .run_dir(&task.id, attempt_n)
            .join(format!("finding-scenario-{id}"));
        let _ = std::fs::create_dir_all(&dir);
        let sandbox = app.task_sandbox(&task.repo, worktree, &dir);
        let results = run_verify_commands(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                worktree,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            std::slice::from_ref(&run),
        )
        .await;
        if cancel.is_cancelled() {
            break;
        }
        if results.first().is_some_and(|v| v.code != Some(0)) {
            crate::task::checks::edits::append_regression(task, &id, &run);
            task.record_decision(format!(
                "Orchestrator: review finding {id} repeated twice and its scenario fails in the worktree; added a criterion and a check: {run}"
            ));
            continue;
        }
        task.record_decision(format!(
            "Orchestrator: review finding {id} repeated twice; its scenario passes in the worktree, so it stays open"
        ));
    }
    if changed {
        task.touch(now_ms());
        app.save_task_logged(task);
    }
}
