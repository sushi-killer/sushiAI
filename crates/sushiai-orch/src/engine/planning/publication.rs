use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::planning::draft::PlanDraft;
use crate::engine::planning::records::store_draft;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::policy::settings::Settings;
use crate::task::attempts::{end, AttemptEnd};
use crate::task::checks::authority::{removal_refusal, Origin};
use crate::task::checks::commands::flat_command;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::state::{Task, TaskStatus};
use std::sync::Arc;

pub(super) struct PlanProposal {
    pub(super) based_on: CheckSnapshot,
    pub(super) draft: PlanDraft,
    pub(super) attempt: usize,
}

pub(super) enum Publication {
    Installed(Box<PlanDraft>),
    Retry,
    Ended,
}

/// An accepted edit is applied to its original contract before the planner may replace it.
pub(super) async fn publish(
    app: &Arc<App>,
    task: &mut Task,
    proposal: PlanProposal,
    settings: &Settings,
    signals: &TaskSignals,
) -> Result<Publication, String> {
    apply_pending_amendment(app, task, &signals.pending_amend, &signals.cancel).await?;
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&task.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    if signals.cancel.is_cancelled()
        || fresh.status != TaskStatus::Drafting
        || fresh.archived
        || fresh.queue.park.is_some()
    {
        *task = fresh;
        return Ok(Publication::Ended);
    }
    let pending = signals.pending_amend.lock().unwrap();
    let outcome = if fresh.pending_amendment.is_some()
        || pending.is_some()
        || !proposal.based_on.matches(&fresh)
    {
        end(
            &mut fresh,
            proposal.attempt,
            AttemptEnd::Interrupted,
            now_ms(),
        );
        fresh.record_decision(
            "Planner: task changed while drafting; the stale draft was not installed".into(),
        );
        Publication::Retry
    } else {
        let mut candidate = fresh.clone();
        store_draft(&mut candidate, proposal.attempt, &proposal.draft, settings);
        let retained: Vec<_> = candidate
            .planner_check_commands()
            .into_iter()
            .map(|(command, _)| flat_command(&command))
            .collect();
        let removed: Vec<_> = fresh
            .planner_check_commands()
            .into_iter()
            .map(|(command, _)| command)
            .filter(|command| !retained.contains(&flat_command(command)))
            .collect();
        let authority = check_authority(&app.settings.read().unwrap(), &fresh);
        if let Some(why) = removal_refusal(&authority, &fresh, &removed, Origin::Other) {
            end(
                &mut fresh,
                proposal.attempt,
                AttemptEnd::Interrupted,
                now_ms(),
            );
            fresh.record_decision(format!("Planner: draft not installed: {why}"));
            fresh.resume_phase(TaskStatus::Stopped, now_ms());
            Publication::Ended
        } else {
            fresh = candidate;
            Publication::Installed(Box::new(proposal.draft))
        }
    };
    fresh.touch(now_ms());
    app.store
        .save_task(&fresh)
        .map_err(|error| error.to_string())?;
    *task = fresh;
    drop(pending);
    drop(_guard);
    app.broadcast_task(task);
    Ok(outcome)
}

#[cfg(test)]
mod tests;
