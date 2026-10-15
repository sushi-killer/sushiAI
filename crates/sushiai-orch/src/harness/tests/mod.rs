use super::{replay_events, RunOutcome, RunRequest};
use crate::policy::routing::Harness;
use std::path::Path;

mod argv;
mod failure;
mod settings;
mod stream;
mod usage;

fn base_req(harness: Harness, worktree: &Path) -> RunRequest<'_> {
    RunRequest {
        harness,
        worktree,
        model: None,
        effort: None,
        max_budget_usd: None,
        review: false,
        task_run: false,
        task_sandbox: None,
        mcp_config: None,
        settings_path: None,
        network_allowed: true,
        codex_mcp: &[],
        images: &[],
        repo_settings: true,
        resume: None,
    }
}

fn claude_stream(lines: &[&str]) -> RunOutcome {
    replay_events(Harness::Claude, &lines.join("\n"))
}
