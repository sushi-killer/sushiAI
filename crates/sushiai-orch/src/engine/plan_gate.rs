//! A typed review and preserved shared checks decide whether a draft needs one replan.

pub(in crate::engine) mod checklist;
pub(in crate::engine) mod grouping;
pub(in crate::engine) mod prompt;
mod publication;
mod review;
mod review_reply;
mod shared_checks;
#[cfg(test)]
mod test_support;

use crate::engine::planning::draft::PlanDraft;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::task::{PlanFinding, Severity};
use std::sync::Arc;

const MAX_FINDING_CHARS: usize = 400;

pub(in crate::engine) enum GateAction {
    Proceed,
    Replan,
    Changed,
    Cancelled,
}

struct PlanReview {
    first_pass: bool,
    removals: Vec<usize>,
    findings: Vec<PlanFinding>,
    decisions: Vec<String>,
}

fn is_critical(finding: &PlanFinding) -> bool {
    finding.severity == Severity::Critical
}

pub(in crate::engine) async fn run(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    draft: &PlanDraft,
    cancel: &CancelToken,
) -> Result<GateAction, String> {
    let task = app
        .store
        .load_task(task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    if cancel.is_cancelled()
        || task.status != crate::task::TaskStatus::Drafting
        || task.archived
        || task.queue.park.is_some()
    {
        return Ok(GateAction::Cancelled);
    }
    if task.pending_amendment.is_some() || task.question.is_some() {
        return Ok(GateAction::Changed);
    }
    let input = publication::GateInput::capture(&task);
    let review = match review::run(app, &task, attempt_n, draft, cancel).await {
        Ok(review) => review,
        Err(RunError::Cancelled) => return Ok(GateAction::Cancelled),
        Err(RunError::Io(error) | RunError::NotFound(error)) => return Err(error),
    };
    publication::GateProposal { input, review }.publish(app, task_id, cancel)
}
