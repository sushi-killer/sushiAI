use crate::clock::now_ms;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::cost::settle_interrupted_review;
use crate::engine::recovery::follow_run;
use crate::engine::run_control::HookContext;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::run_files::RunFiles;
use crate::policy::routing::Route;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::lifecycle::execution::{ExecutedAttempt, RunReceipt};
pub(in crate::engine) async fn follow(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    cancel: &CancelToken,
) -> ExecutedAttempt {
    let task_id = task.id.clone();

    let settings = app.settings.read().unwrap().clone();
    let a = task.attempts[idx].clone();
    let route = settings
        .routes
        .iter()
        .find(|r| {
            r.id == a.route_id
                && r.harness == a.harness
                && r.model.as_deref().unwrap_or_default() == a.model
        })
        .cloned()
        .unwrap_or_else(|| Route {
            id: a.route_id.clone(),
            label: a.route_id.clone(),
            harness: a.harness,
            model: (!a.model.is_empty()).then(|| a.model.clone()),
            effort: None,
            profile_id: None,
            account_id: None,
            strength: None,
        });
    let run_dir = app.store.run_dir(&task_id, a.n);
    // A review it died under is paid for too; the review runs again.
    settle_interrupted_review(task, idx, &app.store, &settings.prices);
    task.resume_phase(TaskStatus::Running, task.updated_at);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    // The agent still running holds the token of its Stop hook: the
    // earlier daemon left it with the run files.
    let worktree = PathBuf::from(&task.worktree);
    let registered = std::fs::read_to_string(&RunFiles::of(&run_dir.join("events.jsonl")).token)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|token| {
            let ctx = HookContext::new(task, a.n, &worktree, &task.base_sha);
            app.register_hook(&token, &ctx, &run_dir);
            (token, ctx)
        });
    let guard = Guard::for_variant(
        &task.variant(),
        registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
    )
    .with_time_budget(settings.implement_budget_mins, a.started_at)
    .with_cap((task.variant().max_attempt_cost_usd > 0.0).then(|| {
        let rejected =
            crate::engine::cost::rejected_resume_cost(task, idx, &app.store, &settings.prices)
                .and_then(|outcome| outcome.cost_usd)
                .unwrap_or(0.0);
        (task.variant().max_attempt_cost_usd - rejected).max(0.0)
    }));
    let result = follow_run(app, task, &a, cancel, guard).await;
    ExecutedAttempt {
        run: RunReceipt {
            settings,
            attempt_n: a.n,
            idx,
            route,
            worktree,
            base_sha: task.base_sha.clone(),
            run_dir: run_dir.clone(),
        },
        key_path: run_dir.join("key"),
        registered,
        run_result: result,
        best_of: BestOfExtra {
            candidates: a.candidates.clone(),
            discarded: task.planner_tests.discarded.clone(),
            ..Default::default()
        },
    }
}
