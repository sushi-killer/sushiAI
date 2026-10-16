use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::task::MissingTool;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::Arc;

use super::judge::{add_task_cost, Judge, JudgePurpose, JudgeRun};
use super::reply::{build_check_brief, parse_missing_tool, parse_verdict, Verdict};
use super::routing::check_route_for;
/// What the caller does after a check.
pub(in crate::engine) enum BriefAction {
    Proceed,
    /// The planner's draft contradicts itself (the conflict text): draft
    /// again, once.
    Redraft(String),
    /// The task was cancelled while the check ran.
    Cancelled,
}

async fn run_check(
    app: &Arc<App>,
    task: &Task,
    route: &Route,
    cancel: &CancelToken,
) -> Result<(Verdict, Option<MissingTool>, Option<String>), RunError> {
    let settings = app.settings.read().unwrap().clone();
    let tools = crate::engine::permissions::tools::task_tools(
        app,
        &settings,
        task,
        Path::new(&task.worktree),
    );
    let brief_text = build_check_brief(&task.goal, &task.criteria, &tools.capabilities());
    let JudgeRun {
        reply,
        cost,
        timeout_note,
    } = Judge::new(route, JudgePurpose::BriefCheck)
        .run(app, task, &brief_text, cancel)
        .await?;
    add_task_cost(app, &task.id, cost);
    Ok(match reply {
        Ok(text) => {
            let missing = parse_missing_tool(&text, task.criteria.len(), &tools.off);
            (parse_verdict(&text), missing, timeout_note)
        }
        Err(why) => (Verdict::Unusable(why), None, timeout_note),
    })
}

/// Checks the stored task's goal and criteria once. `can_redraft` is true
/// for a planner draft: a contradiction then asks the planner to correct it
/// (at most once per task); the caller changes the request only when it has
/// to draft from scratch (see [`redraft_request_note`]). Otherwise, and for an unusable reply, the task
/// carries on; a conflict is kept for the implement and review briefs.
pub(in crate::engine) async fn check_brief(
    app: &Arc<App>,
    task_id: &str,
    can_redraft: bool,
    cancel: &CancelToken,
) -> BriefAction {
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    if task.brief_check.done {
        return BriefAction::Proceed;
    }
    let Some((route_id, route, fallback)) = check_route_for(app, &task.variant()) else {
        return BriefAction::Proceed;
    };
    let (verdict, missing, timeout_note) = match &route {
        Some(route) => match run_check(app, &task, route, cancel).await {
            Ok(v) => v,
            Err(RunError::Cancelled) => return BriefAction::Cancelled,
            Err(RunError::Io(msg) | RunError::NotFound(msg)) => {
                (Verdict::Unusable(msg), None, None)
            }
        },
        None => (
            Verdict::Unusable(format!(
                "no route on an installed agent CLI for {route_id}; the check is off"
            )),
            None,
            None,
        ),
    };
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return BriefAction::Proceed;
    };
    task.record_decisions(timeout_note);
    let mut action = BriefAction::Proceed;
    if let Some(note) = fallback {
        task.record_decision(format!("Brief check: {note}"));
    }
    match verdict {
        Verdict::Clear => task.record_decision("Brief check: no contradiction found".to_string()),
        Verdict::Unusable(why) => task.record_decision(format!(
            "Brief check: no usable answer ({}); continuing",
            truncate_chars(&why, 200)
        )),
        Verdict::Conflict(conflict) => {
            if can_redraft && !task.plan_gate.replanned {
                task.record_decision(format!("Brief check: {conflict} (drafting again)"));
                task.mark_replanned();
                task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
                action = BriefAction::Redraft(conflict);
            } else {
                task.record_decision(format!("Brief check: {conflict}"));
                task.record_brief_conflict(conflict);
            }
        }
    }
    // A redraft is checked again; anything else is settled for good. A tool
    // the task lacks is asked about before the first attempt, once.
    if !matches!(action, BriefAction::Redraft(_)) && !task.brief_check.missing_tool_asked {
        if let Some(missing) = missing {
            task.record_decision(format!(
                "Brief check: criterion {} needs {}, which the task will not have",
                missing.criterion, missing.capability
            ));
            task.record_missing_tool(missing);
        }
    }
    task.finish_brief_check(!matches!(action, BriefAction::Redraft(_)));
    task.touch(now_ms());
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    action
}
