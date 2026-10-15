mod driver;
mod transfer;

use crate::engine::planning::records::end_plan_stage;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::policy::pipelines::planning::PlanningFlow;
use std::sync::Arc;

use driver::PlanningSession;

pub enum PlanOutcome {
    /// Planning completed and the owner requested automatic implementation.
    Proceed,
    /// The completed plan is stopped for the owner's review.
    StoppedForReview,
    /// Planning ended through cancellation, failure or an explicit stop.
    Ended,
}

pub(in crate::engine) async fn run_plan_stage(
    app: &Arc<App>,
    task_id: &str,
    signals: &TaskSignals,
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
    let flow = match PlanningFlow::compiled() {
        Ok(flow) => flow,
        Err(error) => {
            eprintln!("orch: invalid planning pipeline: {error}");
            return end_plan_stage(app, task_id, None).await;
        }
    };
    let initial = match transfer::PlanningInput::Admission.enter(flow.entry()) {
        Ok(initial) => initial,
        Err(error) => {
            eprintln!("orch: invalid planning entry: {error}");
            return end_plan_stage(app, task_id, None).await;
        }
    };
    let mut session = PlanningSession::new(task_id, choice, auto_start_after_plan, flow);
    session.drive(app, signals, permit, initial).await
}
