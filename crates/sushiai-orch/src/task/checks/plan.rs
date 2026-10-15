use crate::task::{valid_check, valid_checks, Check, CheckSource, SourcedCheck, Task};

/// The check contract produced by a completed planner result. Provider
/// parsing and repository classification happen before this operation.
pub(crate) struct CheckPlan {
    pub(crate) criteria: Vec<String>,
    pub(crate) visual_criteria: Vec<String>,
    pub(crate) verify: Vec<String>,
    pub(crate) final_verify: Vec<String>,
    pub(crate) checks: Vec<Check>,
    pub(crate) held_out: Option<Check>,
}

pub(crate) fn install(task: &mut Task, plan: CheckPlan) {
    let task = &mut task.data;
    // An existing command without provenance remains the repo's when a new
    // plan is installed; a later classification cannot make it removable.
    for (command, _) in task.planner_check_commands() {
        if !task
            .planner_tests
            .sources
            .iter()
            .any(|source| source.command == command)
        {
            super::sources::record_source(
                task,
                SourcedCheck {
                    command,
                    source: CheckSource::Repo,
                },
            );
        }
    }
    let len = plan.criteria.len();
    task.criteria = plan.criteria;
    task.visual_criteria = plan.visual_criteria;
    task.verify = plan.verify;
    task.final_verify = plan.final_verify;
    task.checks = valid_checks(plan.checks, len);
    task.held_out = valid_check(plan.held_out, len);
}
