use crate::engine::planning::records::end_plan_stage;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use std::sync::Arc;

use crate::engine::planning::draft_run::DraftRun;
use crate::engine::planning::round::prepare_round;
pub enum PlanOutcome {
    /// Planning finished and `auto_start_after_plan` was set: the task is
    /// now `queued`, fall straight through to the implement stage.
    Proceed,
    /// Planning finished but `auto_start_after_plan` was not set: the task
    /// is `stopped` with everything filled in for the owner to review.
    StoppedForReview,
    /// Cancelled, or ended in `failed`/`stopped` some other way -- the task
    /// loop itself is done, the caller should return.
    Ended,
}

pub(in crate::engine) async fn run_plan_stage(
    app: &Arc<App>,
    task_id: &str,
    signals: &crate::engine::run_control::TaskSignals,
    permit: &mut Option<SlotPermit>,
    auto_start_after_plan: bool,
) -> PlanOutcome {
    let entry_task = match app.store.load_task(task_id) {
        Ok(Some(task)) => task,
        _ => return end_plan_stage(app, task_id, None).await,
    };
    let choice = {
        let settings = app.settings.read().unwrap();
        let variant = entry_task.variant();
        let (id, decision) = variant.plan_route_for(&settings, &entry_task);
        (id.to_string(), decision)
    };
    let mut clarify_rounds = 0;
    loop {
        let Some(mut round) = prepare_round(app, task_id, &choice, signals, permit).await else {
            return end_plan_stage(app, task_id, None).await;
        };
        let draft = match round.draft(signals).await {
            DraftRun::Parsed(draft) => *draft,
            DraftRun::OverBudget => continue,
            DraftRun::Cancelled | DraftRun::Ended => {
                return end_plan_stage(app, task_id, Some(round.index)).await
            }
            DraftRun::Malformed => {
                if let Some(outcome) = round.clarify(&mut clarify_rounds, signals, permit).await {
                    return outcome;
                }
                continue;
            }
        };
        let draft = match round.review_plan(draft, signals).await {
            Ok(Some(draft)) => draft,
            Ok(None) => continue,
            Err(outcome) => return outcome,
        };
        if let Some(outcome) = round.answer_questions(&draft, signals, permit).await {
            return outcome;
        }
        if let Some(outcome) = round.split(&draft, auto_start_after_plan).await {
            return outcome;
        }
        if let Some(outcome) = round.verify_command(signals, permit).await {
            return outcome;
        }
        return round.finish(auto_start_after_plan, signals);
    }
}
