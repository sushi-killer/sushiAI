use super::native::{head_chars, review_request, write_readonly_claude_settings_with_profile};
use super::routing::cheapest_route_at;
use super::run::ReviewSession;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness::guard::Guard;
use crate::policy::routing::{Harness, Route};
use crate::policy::settings::Settings;
use crate::task::Task;
use crate::verify::review::routing::{LensRouting, Lenses};
use std::sync::Arc;

/// The route a route turn runs on: the cheapest configured route, else the
/// review's own route.
fn route_turn_route(settings: &Settings, review_route: &Route) -> Route {
    cheapest_route_at(settings, 0, &[], review_route, false)
        .map_or_else(|| review_route.clone(), |(route, _)| route.clone())
}

/// Asks a read-only agent which review areas this change can break: one cheap
/// turn before a full review. Returns the areas and the decision line that
/// says what was chosen. A turn with no usable answer reviews every area, so
/// a failed route never narrows the review.
pub(super) async fn route_lenses(
    session: &ReviewSession,
    app: &Arc<App>,
    task: &Task,
) -> (Lenses, String, f64) {
    let (result, cost) = route_turn(session, app, task).await;
    let attempt_n = session.attempt_n;
    let (lenses, note) = match result {
        Ok(routing) => {
            let chosen = routing.lenses.chosen();
            let areas = if chosen.is_empty() { "none".to_string() } else { chosen.join(", ") };
            (routing.lenses, format!("Orchestrator: attempt {attempt_n} review areas: {areas} ({})", routing.reason))
        }
        Err(why) => (Lenses::ALL, format!("Orchestrator: attempt {attempt_n} review route gave no areas ({why}); reviewing every area")),
    };
    (lenses, note, cost)
}

async fn route_turn(
    session: &ReviewSession,
    app: &Arc<App>,
    task: &Task,
) -> (Result<LensRouting, String>, f64) {
    let settings = app.settings.read().unwrap().clone();
    let route = route_turn_route(&settings, &session.route);
    let (wt, base) = (session.worktree.clone(), session.base_sha.clone());
    let changed = tokio::task::spawn_blocking(move || {
        crate::git::changes::diff_name_only(&wt, &base).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let brief = crate::verify::review::routing::build_route_brief(task, &changed);
    let dir =
        match app
            .store
            .claim_helper_run_dir(&task.id, "route-review", Some(session.attempt_n))
        {
            Ok(dir) => dir,
            Err(error) => return (Err(error.to_string()), 0.0),
        };
    let mcp = dir.join("mcp.json");
    let _ = std::fs::write(&mcp, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(dir.join("brief.md"), &brief);
    let events = dir.join("events.jsonl");
    let settings_path = dir.join("settings.json");
    let key = dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let request = crate::harness::RunRequest {
        repo_settings: false,
        resume: None,
        ..review_request(&route, &session.worktree, &mcp, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        RunIdentity {
            id: &task.id,
            attempt_n: session.attempt_n,
            track: RunTrack::No,
            tag: CostTag::task("review", &route.id),
        },
        RunInvocation {
            request: &request,
            brief: &brief,
            events: &events,
            schema: None,
        },
        &session.cancel,
        Guard::helper("review route", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(_) => {
            return (
                Err("the route turn was stopped or did not start".to_string()),
                0.0,
            )
        }
    };
    let cost = outcome.cost_usd.unwrap_or(0.0);
    let result = if outcome.timed_out {
        Err("the route turn timed out".to_string())
    } else if let Some(error) = outcome.error {
        Err(format!(
            "the route turn errored: {}",
            head_chars(&error, 200)
        ))
    } else {
        crate::verify::review::routing::parse_lens_routing(&outcome.final_text.unwrap_or_default())
            .ok_or_else(|| "the route turn gave no lenses".to_string())
    };
    (result, cost)
}
