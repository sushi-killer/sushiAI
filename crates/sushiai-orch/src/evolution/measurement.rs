use super::clustering::cluster_key;
use crate::policy::evolution::EvolutionSettings;
use crate::task::evolution::signals::StoredSignal;
use crate::task::evolution::{Proposal, ProposalCounts, ProposalStatus, Track};
use crate::task::{Task, TaskStatus};
use std::collections::HashSet;

/// Adopts an approved repo-track proposal whose task is done, then counts
/// the finished, non-eval tasks of its repo (and the cluster's signals they
/// left) on each side of the adoption, suggesting a revert once enough tasks
/// ran after it without the signals falling. Returns whether anything changed.
pub(super) fn measure(
    p: &mut Proposal,
    tasks: &[Task],
    signals: &[StoredSignal],
    settings: &EvolutionSettings,
) -> bool {
    let before_state = p.clone();
    if p.status == ProposalStatus::Approved && p.track == Track::Repo {
        let done = p
            .task_id
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .filter(|t| t.status == TaskStatus::Done);
        if let Some(task) = done {
            p.status = ProposalStatus::Adopted;
            p.adopted_at = Some(task.updated_at);
        }
    }
    if p.status != ProposalStatus::Adopted {
        return *p != before_state;
    }
    let Some(adopted_at) = p.adopted_at else {
        return *p != before_state;
    };
    let finished: Vec<&Task> = tasks
        .iter()
        .filter(|t| {
            t.repo == p.repo
                && t.eval_set.is_none()
                && matches!(t.status, TaskStatus::Done | TaskStatus::Failed)
                && p.task_id.as_deref() != Some(t.id.as_str())
        })
        .collect();
    let count = |after: bool| {
        let ids: HashSet<&str> = finished
            .iter()
            .filter(|t| (t.updated_at >= adopted_at) == after)
            .map(|t| t.id.as_str())
            .collect();
        let hits = signals
            .iter()
            .filter(|s| {
                ids.contains(s.signal.task_id.as_str()) && cluster_key(&s.signal) == p.cluster_key
            })
            .count();
        ProposalCounts {
            tasks: ids.len() as u32,
            signals: hits as u32,
        }
    };
    let (before, after) = (count(false), count(true));
    p.before = Some(before);
    p.after = Some(after);
    // No tasks before it means nothing to compare the rate to.
    let no_drop = before.tasks > 0
        && after.tasks >= settings.revert_after_tasks
        && u64::from(after.signals) * u64::from(before.tasks)
            >= u64::from(before.signals) * u64::from(after.tasks);
    if no_drop {
        p.status = ProposalStatus::RevertSuggested;
        p.reason = Some(format!(
            "{} matching signal(s) in {} task(s) after adoption against {} in {} before: no drop, consider reverting",
            after.signals, after.tasks, before.signals, before.tasks
        ));
    }
    *p != before_state
}

#[cfg(test)]
mod tests;
