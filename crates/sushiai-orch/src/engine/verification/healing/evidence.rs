use crate::engine::review::evidence::evidence_scope;
use crate::engine::review::evidence::save_evidence;
use crate::engine::state::App;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::task::review::ReviewResult;
use crate::task::state::Task;
use crate::verify::healing::scope::same_failure;
use crate::verify::passes::is_interrupted;
use std::path::Path;
use std::sync::Arc;

/// Runs the planner's screenshot command in the worktree and saves the
/// images the attempt left under `artifacts/`. A failing command is a
/// decision line, never a failed attempt.
pub(in crate::engine) async fn capture_screenshots(
    runner: crate::engine::verification::runner::CheckRunner<'_>,
    task: &mut Task,
    idx: usize,
    run_dir: &Path,
) {
    let crate::engine::verification::runner::CheckRunner {
        ctx,
        cwd: worktree,
        cancel,
        ..
    } = runner;
    let app = ctx.app;
    let Some(cmd) = task.brief_check.screenshot.clone() else {
        return;
    };
    let ran = run_verify_commands(runner, std::slice::from_ref(&cmd)).await;
    record_check_outcomes(app, task, &ran);
    if cancel.is_cancelled() {
        return;
    }
    if let Some(bad) = ran.iter().find(|r| r.code != Some(0)) {
        task.record_decision(format!(
            "Orchestrator: screenshot command {cmd} exited {}; the images it left are used, the attempt is not failed for it",
            bad.code.map(|c| c.to_string()).unwrap_or_else(|| "null".into())
        ));
    }
    let since = task.attempts[idx].started_at;
    let saved = save_evidence(
        worktree,
        since,
        &run_dir.join("evidence"),
        &evidence_scope(task),
    );
    crate::task::attempts::record_evidence(
        task,
        idx,
        crate::task::attempts::EvidenceObservation::Additional(saved),
    );
}

/// The reviewer's own findings that the diff did not cause, shown by their
/// scenario: the reviewer says `causedByDiff` is false and the scenario's
/// `run` fails the same way on the base and on the task tree. A scenario whose
/// target is absent on the base, a planner test or a bare failing command
/// proves nothing about the base: the finding stays blocking.
pub(in crate::engine) async fn findings_old_on_base(
    app: &Arc<App>,
    task: &Task,
    review: &ReviewResult,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Vec<usize> {
    let worktree = Path::new(&task.worktree);
    let own = review.severities.len().min(review.findings.len());
    let mut old = Vec::new();
    for (i, detail) in review.details.iter().enumerate().take(own) {
        let run = detail
            .scenario
            .as_ref()
            .map(|s| s.run.trim().to_string())
            .filter(|run| !run.is_empty());
        let Some(run) = run.filter(|_| detail.caused_by_diff == Some(false)) else {
            continue;
        };
        if task.planner_tests.sources.iter().any(|c| c.command == run)
            || !crate::verify::repository::targets_absent_on_base(&task.repo, &task.base_sha, &run)
                .is_empty()
        {
            continue;
        }
        let ran = crate::engine::verification::base::run::run_on_base(
            app,
            crate::engine::verification::base::jobs::BaseCheckJob::new(
                Some(&task.id),
                &task.repo,
                &task.base_sha,
                &run_dir.join("finding-base"),
                false,
            ),
            std::slice::from_ref(&run),
            cancel,
        )
        .await;
        let Ok(results) = ran else { continue };
        let Some(base) = results
            .first()
            .filter(|r| r.code.is_some_and(|c| c != 0) && !is_interrupted(r))
        else {
            continue;
        };
        let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
        let here = run_verify_commands(
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
        if here
            .first()
            .is_some_and(|r| r.code.is_some_and(|c| c != 0) && same_failure(r, base))
        {
            old.push(i);
        }
    }
    old
}
