use crate::engine::planning::draft::PlanDraft;
use crate::policy::settings::Settings;
use crate::task::checks::plan::CheckPlan;
use crate::task::{valid_check, valid_checks, Task};
use crate::verify::command_gate::{gate_checks, gate_held_out};
use crate::verify::options::split_verify_commands;
use crate::verify::repository::whole_repo_suite;
use std::path::Path;

/// The draft's gated shell checks and the decisions explaining their placement.
pub(super) struct PlannedChecks {
    plan: CheckPlan,
    decisions: Vec<String>,
}

impl PlannedChecks {
    pub(super) fn prepare(task: &Task, draft: &PlanDraft, settings: &Settings) -> Self {
        let worktree = Path::new(&task.worktree);
        let mut decisions = Vec::new();
        // A whole-repo suite is no per-attempt check: it runs once, as a
        // finalVerify, on the final tree. Before the split, so it also applies
        // when review is off.
        let (suites, kept): (Vec<String>, Vec<String>) = draft
            .verify
            .iter()
            .cloned()
            .partition(|c| whole_repo_suite(worktree, c));
        // With review off nobody would check a moved entry: keep it as is.
        let (verify, judged) = if task.variant().review_route_id(settings).is_empty() {
            (kept, Vec::new())
        } else {
            split_verify_commands(worktree, &kept)
        };
        let mut criteria = draft.criteria.clone();
        criteria.extend(
            judged
                .into_iter()
                .map(|command| format!("Checked by review (not a shell command): {command}")),
        );
        let mut finals: Vec<_> = draft
            .final_verify
            .iter()
            .filter(|command| !command.trim().is_empty())
            .cloned()
            .collect();
        for suite in suites {
            if !finals.contains(&suite) {
                finals.push(suite.clone());
            }
            decisions.push(format!(
                "Orchestrator: verify entry {suite} is a whole-repo suite; moved to finalVerify"
            ));
        }
        let mut refused = Vec::new();
        let checks = gate_checks(
            worktree,
            valid_checks(draft.checks.clone(), draft.criteria.len()),
            &mut refused,
        );
        let held_out = gate_held_out(
            worktree,
            valid_check(draft.held_out.clone(), draft.criteria.len()),
            &mut refused,
        );
        decisions.extend(refused);
        let plan = CheckPlan {
            criteria,
            visual_criteria: draft.visual_criteria.clone(),
            verify,
            final_verify: finals,
            checks,
            held_out,
        };
        Self { plan, decisions }
    }

    pub(super) fn install(self, task: &mut Task) {
        task.record_decisions(self.decisions);
        crate::task::checks::plan::install(task, self.plan);
    }
}
