use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness_schema;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::state::Task;
use std::path::Path;
use std::sync::Arc;

/// The cost stage both PR helper turns book under (`store::costs::STAGES`); the body
/// check is part of drafting the pull request.
const STAGE: &str = "pr_draft";

/// What one read-only helper turn runs with.
pub(super) struct HelperRun<'a> {
    /// The turn's run directory, guard and log name.
    pub(super) name: &'static str,
    /// The reply shape the harness enforces for this turn.
    pub(super) schema: &'static str,
    pub(super) route: &'a Route,
    pub(super) settings: &'a Settings,
    pub(super) worktree: &'a Path,
    pub(super) brief: &'a str,
}

/// One read-only agent turn with the brief, in the run's worktree, under the
/// turn's schema. `cost` gets the spend of a run that finished.
pub(super) async fn run_helper(
    app: &Arc<App>,
    task: &Task,
    run: HelperRun<'_>,
    cost: &mut f64,
) -> Result<harness::RunOutcome, String> {
    let HelperRun {
        name,
        schema,
        route,
        settings,
        worktree,
        brief,
    } = run;
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, name, None)
        .map_err(|e| e.to_string())?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), brief);
    let events_path = run_dir.join("events.jsonl");
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        task_run: false,
        task_sandbox: None,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: &[],
        images: &[],
        repo_settings: true,
        resume: None,
    };
    let schema_path = run_dir.join("schema.json");
    let _ = std::fs::write(&schema_path, schema);
    let output = harness::OutputSchema {
        json: schema,
        path: &schema_path,
    };
    let result = run_harness_schema(
        app,
        &task.id,
        0,
        RunTrack::No,
        worktree,
        &req,
        CostTag::task(STAGE, &route.id),
        brief,
        &events_path,
        &CancelToken::new(),
        Guard::helper(name, app.helper_timeout()),
        Some(&output),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => return Err("the run was stopped".to_string()),
        Err(RunError::Io(e) | RunError::NotFound(e)) => return Err(e),
    };
    *cost += outcome.cost_usd.unwrap_or(0.0);
    Ok(outcome)
}
