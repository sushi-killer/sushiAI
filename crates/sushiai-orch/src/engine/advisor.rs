use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::cost::replay_run_cost;
use crate::engine::review::native::{review_request, write_readonly_claude_settings_with_profile};
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::assert_helper_timeout;
#[cfg(test)]
use crate::engine::test_support::slow_helper_task;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::harness::run_files::read_cancelled_fingerprint;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::prompts::clip::truncate_chars;
use crate::task::AttemptStatus;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Longest advice kept, in characters.
const MAX_ADVICE_CHARS: usize = 1500;

/// Why a failed implementation needs advice before its next attempt.
pub(in crate::engine) enum AdviceTrigger<'a> {
    Retry(&'a Route),
    RepeatedFinding,
}

struct AdviceTarget<'a> {
    failed: usize,
    attempt_n: u32,
    failure_detail: String,
    route: &'a Route,
}

impl<'a> AdviceTarget<'a> {
    fn select(task: &Task, settings: &'a Settings, trigger: AdviceTrigger<'_>) -> Option<Self> {
        let failed = task
            .attempts
            .iter()
            .rposition(|a| a.stage == Stage::Implement)?;
        let attempt = &task.attempts[failed];
        let failure = attempt.failure.as_ref()?;
        if attempt.status != AttemptStatus::Failed
            || failure.kind == FailureKind::Blocked
            || attempt.advice.is_some()
            || attempt.advisor_cost_usd.is_some()
        {
            return None;
        }
        let variant = task.variant();
        let planner = variant.plan_route_id(settings);
        let advisor = settings.routes.iter().find(|r| r.id == planner)?;
        if let AdviceTrigger::Retry(next_route) = trigger {
            if advisor.id == next_route.id
                || (advisor.harness == next_route.harness
                    && advisor.model.is_some()
                    && advisor.model == next_route.model)
            {
                return None;
            }
        }
        Some(Self {
            failed,
            attempt_n: attempt.n,
            failure_detail: failure.detail.clone(),
            route: advisor,
        })
    }
}

/// One read-only call on the task's planner route about the implement attempt
/// that just failed, made before the next attempt starts. The answer is
/// stored as the failed attempt's `advice`, and the run's cost as its
/// `advisor_cost_usd`, which is also returned (`None` when no run happened).
/// A failed attempt is advised at most once: a run that was stopped or gave
/// no answer is not paid for again. Never fails the task: no route, an
/// escalation to the advisor's own model, a failed or stopped run or an
/// empty answer all just mean no advice. `RepeatedFinding` allows a fresh diagnosis even
/// when the next attempt uses the planner's route.
pub(super) async fn run_advisor_before_retry(
    app: &Arc<App>,
    task: &mut Task,
    settings: &Settings,
    trigger: AdviceTrigger<'_>,
    cancel: &CancelToken,
) -> Option<f64> {
    let target = AdviceTarget::select(task, settings, trigger)?;
    let worktree = PathBuf::from(&task.worktree);
    let (wt, base) = (worktree.clone(), task.base_sha.clone());
    let diff = tokio::task::spawn_blocking(move || {
        crate::git::changes::diff_full(&wt, &base, 20_000).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let brief_text =
        crate::engine::attempts::advisor::build_advisor_brief(task, &diff, &target.failure_detail);

    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "advisor", Some(target.attempt_n))
        .ok()?;
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(target.route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            target.route,
            app,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = review_request(target.route, &worktree, &mcp_path, &settings_path, &[]);
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        RunIdentity {
            id: &task.id,
            attempt_n: target.attempt_n,
            track: RunTrack::No,
            tag: CostTag::task("advisor", &target.route.id),
        },
        RunInvocation {
            request: &req,
            brief: &brief_text,
            events: &events_path,
            schema: None,
        },
        cancel,
        Guard::helper("advisor", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // The run wrote the attempt's session/pgid to disk mid-run; keep the
    // caller's copy and only add the advice.
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => {
            let cost = replay_run_cost(&events_path, &settings.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            crate::task::attempts::record_advisor(
                task,
                target.failed,
                cost,
                read_cancelled_fingerprint(&events_path),
            );
            return Some(cost);
        }
        Err(RunError::Io(_) | RunError::NotFound(_)) => return None,
    };
    if outcome.timed_out {
        task.record_decision(helper_timeout_message("advisor", limit));
    }
    let cost = outcome.cost_usd.unwrap_or(0.0);
    crate::task::attempts::record_advisor(task, target.failed, cost, outcome.fingerprint.clone());
    let text = outcome.final_text.unwrap_or_default();
    let advice = text.trim();
    if outcome.error.is_none() && !advice.is_empty() {
        crate::task::attempts::record_advice(
            task,
            target.failed,
            truncate_chars(advice, MAX_ADVICE_CHARS),
        );
    }
    Some(cost)
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn advisor_timed_out_gives_no_advice_and_is_not_retried() {
        let (app, _dir, mut task, _route) = slow_helper_task();
        let settings = app.settings.read().unwrap().clone();
        let cancel = CancelToken::new();
        let cost = run_advisor_before_retry(
            &app,
            &mut task,
            &settings,
            AdviceTrigger::RepeatedFinding,
            &cancel,
        )
        .await;
        assert!(cost.is_some());
        assert!(task.attempts[0].advice.is_none());
        assert!(task.attempts[0].advisor_cost_usd.is_some());
        assert!(run_advisor_before_retry(
            &app,
            &mut task,
            &settings,
            AdviceTrigger::RepeatedFinding,
            &cancel
        )
        .await
        .is_none());
        assert_helper_timeout(&task, "advisor");
    }
}
