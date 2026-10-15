use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::cost::replay_run_cost;
use crate::engine::review::native::{review_request, write_readonly_claude_settings_with_profile};
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::{helper_timeout_message, Guard};
use crate::harness::native::error::RunError;
use crate::policy::routing::{Harness, Route};
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(in crate::engine) struct JudgeRun {
    pub reply: Result<String, String>,
    pub cost: f64,
    pub timeout_note: Option<String>,
}

#[derive(Clone, Copy)]
pub(in crate::engine) enum JudgePurpose {
    BriefCheck,
    DependencyRefresh,
    PlanReview(u32),
    AnswerCheck(u32),
    CriterionJudge(u32),
    Feasibility,
}

pub(in crate::engine) struct Judge<'a> {
    route: &'a Route,
    purpose: JudgePurpose,
}

impl<'a> Judge<'a> {
    pub(in crate::engine) fn new(route: &'a Route, purpose: JudgePurpose) -> Self {
        Self { route, purpose }
    }

    pub(in crate::engine) async fn run(
        &self,
        app: &Arc<App>,
        task: &Task,
        prompt: &str,
        cancel: &CancelToken,
    ) -> Result<JudgeRun, RunError> {
        let route = self.route;
        let (attempt_n, stage, name) = match self.purpose {
            JudgePurpose::BriefCheck => (None, "brief_check", "brief-check"),
            JudgePurpose::DependencyRefresh => (None, "refresh", "refresh"),
            JudgePurpose::PlanReview(n) => (Some(n), "plan", "plan-review"),
            JudgePurpose::AnswerCheck(n) => (Some(n), "brief_check", "answer-check"),
            JudgePurpose::CriterionJudge(n) => (Some(n), "brief_check", "criterion-judge"),
            JudgePurpose::Feasibility => (None, "brief_check", "feasibility"),
        };
        let settings = app.settings.read().unwrap().clone();
        let worktree = PathBuf::from(&task.worktree);
        let run_dir = app
            .store
            .claim_helper_run_dir(&task.id, name, attempt_n)
            .map_err(|e| RunError::Io(e.to_string()))?;
        let mcp_path = run_dir.join("mcp.json");
        let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        if matches!(route.harness, Harness::Claude) {
            write_readonly_claude_settings_with_profile(
                route,
                app,
                &settings,
                &deny_read,
                &settings_path,
            );
        }
        let _ = std::fs::write(run_dir.join("brief.md"), prompt);
        let events_path = run_dir.join("events.jsonl");
        let req = harness::RunRequest {
            repo_settings: false,
            resume: None,
            ..review_request(route, &worktree, &mcp_path, &settings_path, &[])
        };
        let limit = app.helper_timeout();
        let result = run_harness(
            app,
            RunIdentity {
                id: &task.id,
                attempt_n: attempt_n.unwrap_or(0),
                track: RunTrack::No,
                tag: if attempt_n.is_some() {
                    CostTag::task(stage, &route.id)
                } else {
                    CostTag::task_without_attempt(stage, &route.id)
                },
            },
            RunInvocation {
                request: &req,
                brief: prompt,
                events: &events_path,
                schema: None,
            },
            cancel,
            Guard::helper(name, limit),
        )
        .await;
        let _ = std::fs::remove_file(&key_path);
        let outcome = match result {
            Ok(o) => o,
            Err(RunError::Cancelled) => {
                // What it streamed before the stop is still spent.
                if let Some(cost) =
                    replay_run_cost(&events_path, &settings.prices).and_then(|o| o.cost_usd)
                {
                    add_task_cost(app, &task.id, cost);
                }
                return Err(RunError::Cancelled);
            }
            Err(e) => return Err(e),
        };
        Ok(JudgeRun::from_outcome(outcome, name, limit))
    }
}

impl JudgeRun {
    fn from_outcome(outcome: harness::RunOutcome, name: &str, limit: std::time::Duration) -> Self {
        let timeout_note = outcome
            .timed_out
            .then(|| helper_timeout_message(name, limit));
        let cost = outcome.cost_usd.unwrap_or(0.0);
        if outcome.timed_out {
            return Self {
                reply: Err("the run timed out".to_string()),
                cost,
                timeout_note,
            };
        }
        if let Some(error) = outcome.error {
            return Self {
                reply: Err(format!("the run failed ({error})")),
                cost,
                timeout_note,
            };
        }
        Self {
            reply: Ok(outcome.final_text.unwrap_or_default()),
            cost,
            timeout_note,
        }
    }
}

pub(in crate::engine) fn add_task_cost(app: &Arc<App>, task_id: &str, cost: f64) {
    if cost <= 0.0 {
        return;
    }
    if let Err(e) = app.update_task(task_id, |task| task.charge(cost)) {
        eprintln!("orch: updating task {task_id} cost failed: {e}");
    }
}
