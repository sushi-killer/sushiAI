use crate::engine::attempts::best_of::candidate::{CandidateRun, CompletedCandidates};
use crate::engine::attempts::best_of::run::BestOfRun;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::prepare::{prepare_run, ImplementRun};
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::cost::replay_run_cost;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::harness::RunOutcome;
use std::path::Path;
use std::sync::Arc;

pub(super) async fn run_candidates<F>(
    app: &Arc<App>,
    run: &BestOfRun<'_>,
    a_run: F,
    sibling: &Sibling,
    cancel: &CancelToken,
) -> (CompletedCandidates, Vec<String>)
where
    F: std::future::Future<Output = Result<RunOutcome, RunError>>,
{
    let primary = &run.primary;
    let secondary = ImplementRun {
        route: run.secondary,
        worktree: &sibling.worktree,
        run_dir: &sibling.run_dir,
        ..*primary
    };
    let _ = std::fs::create_dir_all(secondary.run_dir);
    let tools = crate::engine::permissions::tools::task_tools(
        app,
        secondary.settings,
        secondary.task,
        secondary.worktree,
    );
    let mut notes = Vec::new();
    for line in &tools.notes {
        if !secondary.task.decisions.contains(line) && !notes.contains(line) {
            notes.push(line.clone());
        }
    }
    let prepared = prepare_run(app, &secondary, &tools);
    let _ = std::fs::write(secondary.run_dir.join("brief.md"), run.brief);
    let events_b = secondary.run_dir.join("events.jsonl");
    let request = secondary.request(&prepared, None);
    let b_run = run_harness(
        app,
        RunIdentity {
            id: secondary.task.id.as_str(),
            attempt_n: secondary.attempt_n,
            track: RunTrack::No,
            tag: CostTag::task("implement", &secondary.route.id),
        },
        RunInvocation {
            request: &request,
            brief: run.brief,
            events: &events_b,
            schema: None,
        },
        cancel,
        secondary.guard(&prepared),
    );
    let (a_res, b_res) = tokio::join!(a_run, b_run);
    if let Some((token, ctx)) = &prepared.registered {
        app.forget_hook(token, secondary.run_dir);
        ctx.cancel.cancel();
        crate::engine::permissions::staging::finish_staging(ctx);
    }
    let _ = std::fs::remove_file(&prepared.key_path);
    let _ = std::fs::remove_file(&prepared.mcp_path);
    let cost_a = run_cost(app, &a_res, &primary.run_dir.join("events.jsonl"));
    let cost_b = run_cost(app, &b_res, &events_b);
    (
        CompletedCandidates {
            a: CandidateRun {
                route: primary.route.clone(),
                worktree: primary.worktree.to_path_buf(),
                run_dir: primary.run_dir.to_path_buf(),
                outcome: a_res,
                cost: cost_a,
            },
            b: CandidateRun {
                route: secondary.route.clone(),
                worktree: secondary.worktree.to_path_buf(),
                run_dir: secondary.run_dir.to_path_buf(),
                outcome: b_res,
                cost: cost_b,
            },
        },
        notes,
    )
}

fn run_cost(app: &App, result: &Result<RunOutcome, RunError>, events: &Path) -> f64 {
    match result {
        Ok(outcome) => outcome.cost_usd.unwrap_or(0.0),
        Err(RunError::Cancelled) => replay_run_cost(events, &app.settings.read().unwrap().prices)
            .and_then(|outcome| outcome.cost_usd)
            .unwrap_or(0.0),
        Err(RunError::Io(_) | RunError::NotFound(_)) => 0.0,
    }
}
