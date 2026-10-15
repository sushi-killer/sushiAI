use crate::clock::now_ms;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::decision::record_failure;
use crate::engine::planning::publication::{publish, PlanProposal, Publication};
use crate::engine::planning::records::write_planner_run_files;
use crate::engine::review::review_request;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Why a brief-check correction produced no draft.
pub(in crate::engine) enum Uncorrected {
    /// Draft from scratch instead; the reason is for the decision line.
    Fresh(String),
    /// The task was cancelled; the plan attempt at this index is open.
    Cancelled(usize),
    Failed,
    Superseded,
    Ended,
}

/// A brief-check contradiction in plan attempt `idx`'s draft: resumes that
/// attempt's planner session with a short correction turn and stores the
/// corrected draft. No session, a failed run or an unparseable reply is
/// `Fresh`; a timed-out planner fails the task.
pub(in crate::engine) async fn correct_draft(
    app: &Arc<App>,
    task_id: &str,
    idx: usize,
    route: &Route,
    conflict: &str,
    signals: &TaskSignals,
) -> Result<crate::engine::planning::draft::PlanDraft, Uncorrected> {
    let cancel = &signals.cancel;
    let fresh = |why: &str| Uncorrected::Fresh(why.to_string());
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return Err(fresh("the task could not be read"));
    };
    let Some(session) = task.attempts.get(idx).and_then(|a| a.session_id.clone()) else {
        return Err(fresh("the planner attempt has no session id"));
    };
    if task.cost_budget().is_some_and(|b| task.cost_usd >= b) {
        return Err(fresh("the cost budget is reached"));
    }
    let settings = app.settings.read().unwrap().clone();
    let attempt_n = task.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
    let at =
        crate::task::attempts::start(&mut task, Stage::Plan, route, "brief correction", now_ms());
    app.save_task_logged(&task);
    app.broadcast_task(&task);

    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(task_id, attempt_n).join("plan");
    let _ = std::fs::create_dir_all(&run_dir);
    let (mcp_path, settings_path, key_path) =
        write_planner_run_files(app, route, &settings, &run_dir);
    let req = harness::RunRequest {
        resume: Some(&session),
        ..review_request(route, &worktree, &mcp_path, &settings_path, &[])
    };
    let brief_text = crate::engine::planning::prompt::build_plan_correction_brief(
        conflict,
        &task.variant(),
        task.parent.is_none(),
    );
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let limit = app.planner_timeout();
    let based_on = CheckSnapshot::capture(&task);
    let result = run_harness(
        app,
        task_id,
        attempt_n,
        RunTrack::Attempt,
        &worktree,
        &req,
        CostTag::task("plan", &route.id),
        &brief_text,
        &run_dir.join("events.jsonl"),
        cancel,
        Guard::helper("planner", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
        task = reloaded;
    }
    let o = match result {
        Ok(o) => o,
        Err(RunError::Cancelled) => return Err(Uncorrected::Cancelled(at)),
        Err(RunError::Io(msg) | RunError::NotFound(msg)) => {
            if cancel.is_cancelled() || task.status != crate::task::TaskStatus::Drafting {
                return Err(Uncorrected::Cancelled(at));
            }
            return correction_failed(
                app,
                &mut task,
                at,
                FailureKind::Error,
                format!("the resumed run could not start: {msg}"),
            );
        }
    };
    let observation = correction_observation(route.harness, &o, &settings.prices);
    task = app
        .update_task(task_id, |task| {
            crate::task::attempts::record_plan_run(task, at, observation);
        })
        .map_err(|_| Uncorrected::Ended)?;
    if cancel.is_cancelled() || task.status != crate::task::TaskStatus::Drafting {
        return Err(Uncorrected::Cancelled(at));
    }
    if o.timed_out {
        return Err(settle_correction_timeout(
            app,
            &mut task,
            at,
            o.error.clone(),
            limit,
        ));
    }
    let draft = match correction_reply(o) {
        Ok(draft) => draft,
        Err((kind, why)) => return correction_failed(app, &mut task, at, kind, why),
    };
    let proposal = PlanProposal {
        based_on,
        draft,
        attempt: at,
    };
    match publish(app, &mut task, proposal, &settings, signals).await {
        Ok(Publication::Installed(draft)) => Ok(*draft),
        Ok(Publication::Retry) => Err(Uncorrected::Superseded),
        Ok(Publication::Ended)
            if task.attempts[at].status == crate::task::AttemptStatus::Running =>
        {
            Err(Uncorrected::Cancelled(at))
        }
        Ok(Publication::Ended) | Err(_) => Err(Uncorrected::Ended),
    }
}

fn settle_correction_timeout(
    app: &Arc<App>,
    task: &mut Task,
    at: usize,
    error: Option<String>,
    limit: std::time::Duration,
) -> Uncorrected {
    task.record_decision(helper_timeout_message("planner", limit));
    record_failure(task, at, FailureKind::Error, error.unwrap_or_default());
    task.mark_failed();
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    Uncorrected::Failed
}

fn correction_failed(
    app: &Arc<App>,
    task: &mut Task,
    at: usize,
    kind: FailureKind,
    why: String,
) -> Result<crate::engine::planning::draft::PlanDraft, Uncorrected> {
    record_failure(task, at, kind, why.clone());
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    Err(Uncorrected::Fresh(why))
}

fn correction_reply(
    outcome: harness::RunOutcome,
) -> Result<crate::engine::planning::draft::PlanDraft, (FailureKind, String)> {
    if let Some(error) = outcome.error {
        return Err((
            FailureKind::Error,
            format!("the resumed run failed ({error})"),
        ));
    }
    crate::engine::planning::draft::parse_plan(&outcome.final_text.unwrap_or_default()).ok_or_else(
        || {
            (
                FailureKind::NoDeliverable,
                "the resumed reply had no valid sushi-plan block".to_string(),
            )
        },
    )
}
fn correction_observation(
    harness: Harness,
    outcome: &harness::RunOutcome,
    prices: &std::collections::BTreeMap<String, crate::policy::pricing::Price>,
) -> crate::task::attempts::PlanRunObservation {
    // A resumed Claude session's own total also covers the run before it.
    let own = match harness {
        Harness::Claude => outcome.streamed_cost(prices),
        Harness::Codex => None,
    };
    crate::task::attempts::PlanRunObservation {
        cost: own.or(outcome.cost_usd),
        estimated: outcome.cost_estimated,
        fingerprint: outcome.fingerprint.clone(),
        usage: None,
    }
}
