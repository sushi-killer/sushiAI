use crate::clock::now_ms;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::prepare::prepare_run;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::Task;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_candidates<F>(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    a_run: F,
    b_route: &Route,
    settings: &Settings,
    sibling: &Sibling,
    brief_text: &str,
    cancel: &CancelToken,
    extra: &mut BestOfExtra,
) -> (
    Result<harness::RunOutcome, RunError>,
    Result<harness::RunOutcome, RunError>,
)
where
    F: std::future::Future<Output = Result<harness::RunOutcome, RunError>>,
{
    let _ = std::fs::create_dir_all(&sibling.run_dir);
    let tools_b =
        crate::engine::permissions::tools::task_tools(app, settings, task, &sibling.worktree);
    for line in &tools_b.notes {
        if !task.decisions.contains(line) && !extra.decisions.contains(line) {
            extra.decisions.push(line.clone());
        }
    }
    let prep = prepare_run(
        app,
        task,
        task_id,
        b_route,
        settings,
        &sibling.worktree,
        attempt_n,
        &sibling.run_dir,
        &tools_b,
    );
    let _ = std::fs::write(sibling.run_dir.join("brief.md"), brief_text);
    let events_b = sibling.run_dir.join("events.jsonl");
    let variant = task.variant();
    let started_at = task
        .attempts
        .iter()
        .rev()
        .find(|a| a.n == attempt_n)
        .map_or_else(now_ms, |a| a.started_at);
    let codex_mcp_b =
        tools_b.codex_servers((prep.sandbox.is_none()).then_some(&prep.messages_server));
    let req_b = harness::RunRequest {
        harness: b_route.harness,
        worktree: &sibling.worktree,
        model: b_route.model.as_deref(),
        effort: b_route.effort.as_deref(),
        max_budget_usd: (variant.max_attempt_cost_usd > 0.0)
            .then_some(variant.max_attempt_cost_usd),
        review: false,
        task_run: true,
        task_sandbox: prep.sandbox.as_ref(),
        mcp_config: Some(&prep.mcp_path),
        settings_path: Some(&prep.settings_path),
        network_allowed: settings.codex_network,
        codex_mcp: &codex_mcp_b,
        images: &[],
        repo_settings: true,
        resume: None,
    };
    let b_run = run_harness(
        app,
        task_id,
        attempt_n,
        RunTrack::No,
        &sibling.worktree,
        &req_b,
        CostTag::task("implement", &b_route.id),
        brief_text,
        &events_b,
        cancel,
        Guard::for_variant(
            &variant,
            prep.registered
                .as_ref()
                .map(|(_, ctx)| ctx.hook_running.clone()),
        )
        .with_time_budget(settings.implement_budget_mins, started_at),
    );
    let results = tokio::join!(a_run, b_run);
    if let Some((token, ctx)) = &prep.registered {
        app.forget_hook(token, &sibling.run_dir);
        ctx.cancel.cancel();
        crate::engine::permissions::staging::finish_staging(ctx);
    }
    let _ = std::fs::remove_file(&prep.key_path);
    let _ = std::fs::remove_file(&prep.mcp_path);

    results
}
