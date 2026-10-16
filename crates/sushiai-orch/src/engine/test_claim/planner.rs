use crate::engine::attempts::run::{RunIdentity, RunInvocation};

use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review::native::head_chars;
use crate::engine::review::native::review_request;
use crate::engine::review::native::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::policy::routing::Harness;
use crate::task::Task;
use std::sync::Arc;

use super::run::ClaimReview;

impl ClaimReview {
    pub(super) async fn planner_run(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        kind: &'static str,
        brief_text: &str,
    ) -> Result<String, String> {
        let attempt_n = self.attempt_n;
        let worktree = self.scratch.as_path();
        let cancel = &self.cancel;
        let settings = app.settings.read().unwrap().clone();
        let route = settings
            .routes
            .iter()
            .find(|r| r.id == task.variant().plan_route_id(&settings))
            .cloned()
            .ok_or("no planner route")?;
        let run_dir = app
            .store
            .claim_helper_run_dir(&task.id, kind, Some(attempt_n))
            .map_err(|e| e.to_string())?;
        let mcp_path = run_dir.join("mcp.json");
        let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
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
        let _ = std::fs::write(run_dir.join("brief.md"), brief_text);
        let events_path = run_dir.join("events.jsonl");
        let req = harness::RunRequest {
            repo_settings: false,
            resume: None,
            ..review_request(&route, worktree, &mcp_path, &settings_path, &[])
        };
        let limit = app.helper_timeout();
        let result = run_harness(
            app,
            RunIdentity {
                id: &task.id,
                attempt_n,
                track: RunTrack::No,
                tag: CostTag::task("finding_judge", &route.id),
            },
            RunInvocation {
                request: &req,
                brief: brief_text,
                events: &events_path,
                schema: None,
            },
            cancel,
            Guard::helper(kind, limit),
        )
        .await;
        let _ = std::fs::remove_file(&key_path);
        let outcome = result.map_err(|_| "the run was stopped or did not start".to_string())?;
        complete(app, task, outcome, kind, limit)
    }
}

fn complete(
    app: &App,
    task: &mut Task,
    outcome: harness::RunOutcome,
    kind: &str,
    limit: std::time::Duration,
) -> Result<String, String> {
    let cost = outcome.cost_usd.unwrap_or(0.0);
    if cost != 0.0 {
        if let Err(error) = app.update_task(&task.id, |fresh| fresh.charge(cost)) {
            eprintln!(
                "orch: updating task {} claim helper cost failed: {error}",
                task.id
            );
        }
        task.charge(cost);
    }
    if outcome.timed_out {
        return Err(helper_timeout_message(kind, limit));
    }
    if let Some(error) = outcome.error {
        return Err(format!("errored: {}", head_chars(&error, 200)));
    }
    Ok(outcome.final_text.unwrap_or_default())
}

#[cfg(test)]
#[path = "tests/settlement.rs"]
mod settlement_tests;
