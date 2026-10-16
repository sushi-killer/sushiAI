use super::native::review_request;
use super::run::ReviewSession;
use super::types::ReviewFailure;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::cost::replay_run_cost;
use crate::engine::state::App;
use crate::harness::guard::{helper_timeout_message, Guard};
use crate::harness::native::error::RunError;
use crate::harness::run_files::read_cancelled_fingerprint;
use crate::policy::routing::Harness;
use crate::task::Fingerprint;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct ReviewExecution {
    pub(super) outcome: Result<crate::harness::RunOutcome, ReviewFailure>,
    pub(super) cost_usd: f64,
    pub(super) fingerprint: Option<Fingerprint>,
    pub(super) timeout_note: Option<String>,
}

impl ReviewExecution {
    pub(super) fn completed(
        outcome: crate::harness::RunOutcome,
        limit: std::time::Duration,
    ) -> Self {
        let cost_usd = outcome.cost_usd.unwrap_or(0.0);
        let fingerprint = outcome.fingerprint.clone();
        let timeout_note = outcome
            .timed_out
            .then(|| helper_timeout_message("reviewer", limit));
        let outcome = if outcome.timed_out {
            Err(ReviewFailure::Harness(format!(
                "review did not run ({})",
                outcome.error.as_deref().unwrap_or_default()
            )))
        } else {
            Ok(outcome)
        };
        ReviewExecution {
            outcome,
            cost_usd,
            fingerprint,
            timeout_note,
        }
    }

    fn failed(why: ReviewFailure) -> Self {
        Self {
            outcome: Err(why),
            cost_usd: 0.0,
            fingerprint: None,
            timeout_note: None,
        }
    }
}

impl ReviewSession {
    pub(super) async fn execute(
        &self,
        app: &Arc<App>,
        task_id: &str,
        brief: &str,
        images: &[PathBuf],
    ) -> ReviewExecution {
        let run_dir = match app
            .store
            .claim_helper_run_dir(task_id, "review", Some(self.attempt_n))
        {
            Ok(dir) => dir,
            Err(error) => {
                return ReviewExecution::failed(ReviewFailure::Harness(error.to_string()))
            }
        };
        let mcp_path = run_dir.join("mcp.json");
        let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
        let _ = std::fs::write(run_dir.join("brief.md"), brief);
        let events_path = run_dir.join("events.jsonl");
        let settings = app.settings.read().unwrap().clone();
        let settings_path = run_dir.join("settings.json");
        if matches!(self.route.harness, Harness::Claude) {
            let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
            let value = crate::harness::build_claude_settings(
                None,
                settings.sandbox,
                &settings.allowed_domains,
                &deny_read,
                None,
            );
            let _ = crate::store::files::write_json_atomic(&settings_path, &value);
        }
        let request = review_request(
            &self.route,
            &self.worktree,
            &mcp_path,
            &settings_path,
            images,
        );
        let limit = app.helper_timeout();
        let result = run_harness(
            app,
            RunIdentity {
                id: task_id,
                attempt_n: self.attempt_n,
                track: RunTrack::No,
                tag: CostTag::task("review", &self.route.id),
            },
            RunInvocation {
                request: &request,
                brief,
                events: &events_path,
                schema: None,
            },
            &self.cancel,
            Guard::helper("reviewer", limit),
        )
        .await;
        match result {
            Ok(outcome) => ReviewExecution::completed(outcome, limit),
            Err(RunError::Cancelled) => ReviewExecution {
                outcome: Err(ReviewFailure::Cancelled),
                cost_usd: replay_run_cost(&events_path, &settings.prices)
                    .and_then(|o| o.cost_usd)
                    .unwrap_or(0.0),
                fingerprint: read_cancelled_fingerprint(&events_path),
                timeout_note: None,
            },
            Err(RunError::Io(why) | RunError::NotFound(why)) => {
                ReviewExecution::failed(ReviewFailure::Harness(why))
            }
        }
    }
}
