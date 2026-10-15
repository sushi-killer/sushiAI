use crate::engine::attempts::best_of::candidates::run_candidates;
use crate::engine::attempts::best_of::evidence::evaluate_pair;
use crate::engine::attempts::best_of::outcome::finish;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::best_of::selection::select;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::cost::replay_run_cost;
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

fn run_cost(app: &App, res: &Result<harness::RunOutcome, RunError>, events: &Path) -> f64 {
    match res {
        Ok(o) => o.cost_usd.unwrap_or(0.0),
        Err(RunError::Cancelled) => replay_run_cost(events, &app.settings.read().unwrap().prices)
            .and_then(|o| o.cost_usd)
            .unwrap_or(0.0),
        Err(RunError::Io(_) | RunError::NotFound(_)) => 0.0,
    }
}

/// Runs the first implement attempt twice at once: `a_run` in the task's
/// worktree and the same brief on `b_route` in a sibling worktree. The
/// winner's changes end up in the task's worktree, the sibling and its
/// branch are removed, and the winner's run is returned as if it were the
/// only one.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn run_best_of<F>(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    a_run: F,
    a_route: &Route,
    b_route: &Route,
    settings: &Settings,
    worktree: &Path,
    base_sha: &str,
    run_dir: &Path,
    brief_text: &str,
    deny_read: &[String],
    cancel: &CancelToken,
) -> (Result<harness::RunOutcome, RunError>, BestOfExtra)
where
    F: std::future::Future<Output = Result<harness::RunOutcome, RunError>>,
{
    let mut extra = BestOfExtra::default();
    let sibling = match Sibling::create(app, task, run_dir, cancel).await {
        Ok(sibling) => sibling,
        Err(error) => {
            extra.decisions.push(format!(
                "Best-of: could not create the sibling worktree ({error}); ran a single candidate"
            ));
            return (a_run.await, extra);
        }
    };
    let (a_res, b_res) = run_candidates(
        app, task, task_id, attempt_n, a_run, b_route, settings, &sibling, brief_text, cancel,
        &mut extra,
    )
    .await;
    let cost_a = run_cost(app, &a_res, &run_dir.join("events.jsonl"));
    let cost_b = run_cost(app, &b_res, &sibling.run_dir.join("events.jsonl"));
    if matches!(a_res, Err(RunError::Cancelled)) || matches!(b_res, Err(RunError::Cancelled)) {
        extra.cost = cost_b;
        sibling.cleanup().await;
        return (Err(RunError::Cancelled), extra);
    }
    let evaluated = evaluate_pair(
        app,
        task,
        worktree,
        base_sha,
        run_dir,
        &sibling,
        &a_res,
        &b_res,
        settings,
        cancel,
        &mut extra.decisions,
    )
    .await;
    let (mut pick_b, mut decision) = match select(
        app, task, task_id, attempt_n, settings, a_route, b_route, worktree, &sibling, &evaluated,
        &mut extra, deny_read, cancel,
    )
    .await
    {
        Ok(selection) => selection,
        Err(_) => {
            extra.cost += cost_b;
            sibling.cleanup().await;
            return (Err(RunError::Cancelled), extra);
        }
    };
    if pick_b {
        if let Err(error) =
            crate::delivery::candidates::carry(worktree, &sibling.worktree, &task.base_sha).await
        {
            pick_b = false;
            decision.push_str(&format!(
                "; could not carry b into the task worktree ({error}), kept a"
            ));
        }
    }
    sibling.cleanup().await;
    extra.decisions.push(decision);
    finish(
        extra, a_res, b_res, a_route, b_route, cost_a, cost_b, evaluated, pick_b,
    )
}
