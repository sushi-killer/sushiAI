use super::shared_checks::shared_check_findings;
use super::{is_critical, GateAction, PlanReview};
use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::answer_policy::authority::{check_authority, is_shared_check};
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::checks::authority::Origin;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::planning::PlanGate;
use crate::task::{Task, TaskStatus};
use std::sync::Arc;

pub(super) struct GateInput {
    checks: CheckSnapshot,
    gate: PlanGate,
}

impl GateInput {
    pub(super) fn capture(task: &Task) -> Self {
        Self {
            checks: CheckSnapshot::capture(task),
            gate: task.plan_gate.clone(),
        }
    }

    fn matches(&self, task: &Task) -> bool {
        self.checks.matches(task) && self.gate == task.plan_gate
    }
}

pub(super) struct GateProposal {
    pub(super) input: GateInput,
    pub(super) review: PlanReview,
}

impl GateProposal {
    pub(super) fn publish(
        mut self,
        app: &Arc<App>,
        task_id: &str,
        cancel: &CancelToken,
    ) -> Result<GateAction, String> {
        let lock = app.task_lock(task_id);
        let guard = lock.lock().unwrap();
        let mut fresh = app
            .store
            .load_task(task_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if cancel.is_cancelled()
            || fresh.status != TaskStatus::Drafting
            || fresh.archived
            || fresh.queue.park.is_some()
        {
            return Ok(GateAction::Cancelled);
        }
        if fresh.pending_amendment.is_some()
            || fresh.question.is_some()
            || !self.input.matches(&fresh)
        {
            return Ok(GateAction::Changed);
        }
        let settings = app.settings.read().unwrap().clone();
        self.review
            .install_report(&mut fresh, &settings, &self.input.checks)?;
        let action = self.decide(&mut fresh, &settings);
        fresh.touch(now_ms());
        app.store.save_task(&fresh).map_err(|e| e.to_string())?;
        drop(guard);
        app.broadcast_task(&fresh);
        Ok(action)
    }

    fn decide(self, task: &mut Task, settings: &Settings) -> GateAction {
        let shared = |command: &str| is_shared_check(settings, task, command);
        let mut findings = shared_check_findings(
            task.request.as_deref().unwrap_or_default(),
            &task.verify,
            &task.final_verify,
            &shared,
        );
        findings.extend(self.review.findings);
        let critical = findings.iter().filter(|f| is_critical(f)).count();
        let replan = critical > 0 && !task.plan_gate.replanned;
        if replan {
            let mut kinds: Vec<_> = findings
                .iter()
                .filter(|f| is_critical(f))
                .map(|f| f.kind.clone())
                .collect();
            kinds.dedup();
            task.record_decision(format!(
                "Plan gate: {critical} critical finding(s) ({}); planning again once",
                kinds.join(", ")
            ));
            task.mark_replanned();
            task.finish_brief_check(false);
            task.clear_missing_tool();
            task.resume_phase(TaskStatus::Drafting, now_ms());
        } else if !findings.is_empty() {
            task.record_decision(format!(
                "Plan gate: {} finding(s) left for the implementer's brief",
                findings.len()
            ));
        }
        task.record_plan_findings(findings);
        if replan {
            GateAction::Replan
        } else {
            GateAction::Proceed
        }
    }
}

impl PlanReview {
    fn install_report(
        &mut self,
        task: &mut Task,
        settings: &Settings,
        snapshot: &CheckSnapshot,
    ) -> Result<(), String> {
        if !self.removals.is_empty() {
            let authority = check_authority(settings, task);
            for index in &self.removals {
                if let Some(criterion) = task.criteria.get(*index) {
                    let text = criterion
                        .split_once("-- check:")
                        .map_or(criterion.as_str(), |(text, _)| text);
                    self.decisions.push(format!(
                        "Plan gate: removed the report-only criterion \"{}\"",
                        truncate_chars(text.trim(), 160)
                    ));
                }
            }
            crate::task::checks::criteria::remove_report_only(
                &authority,
                task,
                snapshot,
                &self.removals,
                Origin::Other,
            )?;
        }
        task.record_decisions(std::mem::take(&mut self.decisions));
        if self.first_pass {
            task.mark_plan_reviewed();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
