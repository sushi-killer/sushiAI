use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{FreshBase, PrPlan, Task, TaskStatus};

/// Publishes the resolution request only while the checked done task still owns it.
pub(super) fn admit(
    app: &App,
    before: &Task,
    tip: String,
    plan: Option<PrPlan>,
) -> Result<Option<Task>, String> {
    let expected = CheckAdmission::capture(before);
    let mut accepted = false;
    let fresh = app.update_task(&before.id, |fresh| {
        if fresh.status != TaskStatus::Done
            || fresh.archived
            || fresh.queue.park.is_some()
            || fresh.pending_amendment.is_some()
            || !fresh.pr_push_owed()
            || fresh.landed_sha.is_some()
            || !expected.matches(fresh)
        {
            return;
        }
        fresh.record_fresh_base(FreshBase { tip: Some(tip) });
        fresh.mark_pr_conflict();
        if plan.is_some() {
            fresh.record_pr_pending(true);
            fresh.defer_pr(plan);
        }
        fresh.resume_phase(TaskStatus::Queued, now_ms());
        accepted = true;
    })?;
    Ok(accepted.then_some(fresh))
}
