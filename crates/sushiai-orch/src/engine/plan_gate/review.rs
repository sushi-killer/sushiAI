use super::prompt::build_review_prompt;
use super::review_reply::{parse_review, split_review};
use super::shared_checks::owner_text;
use super::{is_critical, PlanReview};
use crate::engine::brief_check::judge::{Judge, JudgePurpose, JudgeRun};
use crate::engine::planning::draft::PlanDraft;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::prompts::clip::truncate_chars;
use crate::task::Task;
use std::sync::Arc;

/// The plan review's route and where it came from: the owner's `planReview`
/// when it names a configured route on an available harness, else the
/// planner's route. `Err` only when neither is usable.
struct SelectedReview {
    route: Route,
    source: String,
}

fn review_route(app: &App, task: &Task) -> Result<SelectedReview, String> {
    let settings = app.settings.read().unwrap().clone();
    let id = settings.plan_review.trim().to_string();
    let why = if id.is_empty() {
        "planReview is empty".to_string()
    } else {
        match settings.routes.iter().find(|r| r.id == id) {
            None => format!("route {id} is not configured"),
            Some(r) if !app.harness_available(r.harness) => {
                format!("the agent CLI of route {id} is not available")
            }
            Some(r) => {
                return Ok(SelectedReview {
                    route: r.clone(),
                    source: "planReview".into(),
                })
            }
        }
    };
    let variant = task.variant();
    let planner = variant.plan_route_id(&settings);
    match settings.routes.iter().find(|r| r.id == planner) {
        Some(r) if app.harness_available(r.harness) => Ok(SelectedReview {
            route: r.clone(),
            source: format!("the planner's route, since {why}"),
        }),
        _ => Err(format!("{why}, and the planner's route is not available")),
    }
}

pub(super) async fn run(
    app: &Arc<App>,
    task: &Task,
    attempt_n: u32,
    draft: &PlanDraft,
    cancel: &CancelToken,
) -> Result<PlanReview, RunError> {
    let mut review = PlanReview {
        first_pass: !task.plan_gate.reviewed,
        removals: Vec::new(),
        findings: Vec::new(),
        decisions: Vec::new(),
    };
    if !review.first_pass {
        review.findings = task
            .plan_gate
            .findings
            .iter()
            .filter(|f| f.id.starts_with("review-") && !is_critical(f))
            .cloned()
            .collect();
        return Ok(review);
    }
    let selected = match review_route(app, task) {
        Ok(selected) => selected,
        Err(why) => {
            review
                .decisions
                .push(format!("Plan gate: plan review skipped, {why}; continuing"));
            return Ok(review);
        }
    };
    let prompt = build_review_prompt(
        task,
        owner_text(task.request.as_deref().unwrap_or_default()),
        &draft.subtasks,
    );
    match Judge::new(&selected.route, JudgePurpose::PlanReview(attempt_n))
        .run(app, task, &prompt, cancel)
        .await
    {
        Err(RunError::Cancelled) => return Err(RunError::Cancelled),
        Err(RunError::Io(msg) | RunError::NotFound(msg)) => review.decisions.push(format!(
            "Plan gate: plan review failed ({}); continuing",
            truncate_chars(&msg, 200)
        )),
        Ok(run) => return selected.complete(app, task, run),
    }
    Ok(review)
}

impl SelectedReview {
    fn complete(self, app: &Arc<App>, task: &Task, run: JudgeRun) -> Result<PlanReview, RunError> {
        let JudgeRun {
            reply,
            cost,
            timeout_note,
        } = run;
        if cost > 0.0 {
            app.update_task(&task.id, |fresh| fresh.charge(cost))
                .map_err(RunError::Io)?;
        }
        let mut review = PlanReview {
            first_pass: true,
            removals: Vec::new(),
            findings: Vec::new(),
            decisions: timeout_note.into_iter().collect(),
        };
        match reply.and_then(|text| parse_review(&text, task.criteria.len())) {
            Ok(found) => {
                review.decisions.push(format!(
                    "Plan gate: plan review by {} ({}) found {} problem(s)",
                    self.route.id,
                    self.source,
                    found.len()
                ));
                let settings = app.settings.read().unwrap().clone();
                (review.removals, review.findings) = split_review(&settings, task, &found);
            }
            Err(why) => review.decisions.push(format!(
                "Plan gate: plan review gave no usable answer ({}); continuing",
                truncate_chars(&why, 200)
            )),
        }
        Ok(review)
    }
}

#[cfg(test)]
mod tests;
