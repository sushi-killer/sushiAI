use super::authority::{criterion_text_refusal, removal_refusal, CheckAuthority, Origin};
use crate::task::{Baseline, Check, PlannerTests, SourcedCheck, Task};

pub(crate) fn rewrite_criterion(
    authority: &CheckAuthority,
    task: &mut Task,
    index: usize,
    text: &str,
) -> Result<(), String> {
    let task = &mut task.data;
    if let Some(why) = criterion_text_refusal(authority, task, index, text) {
        return Err(why);
    }
    let Some(criterion) = task.criteria.get_mut(index) else {
        return Err("criterion is out of range".to_string());
    };
    let old = std::mem::replace(criterion, text.to_string());
    for visual in task
        .visual_criteria
        .iter_mut()
        .filter(|visual| **visual == old)
    {
        *visual = text.to_string();
    }
    Ok(())
}

pub(crate) fn rewrite_command(
    authority: &CheckAuthority,
    task: &mut Task,
    command: &str,
    replacement: &str,
    origin: Origin,
) -> Result<(), String> {
    let task = &mut task.data;
    if let Some(why) = removal_refusal(authority, task, &[command.to_string()], origin) {
        return Err(why);
    }
    for current in task
        .verify
        .iter_mut()
        .chain(&mut task.final_verify)
        .filter(|current| current.as_str() == command)
    {
        *current = replacement.to_string();
    }
    for check in task
        .checks
        .iter_mut()
        .chain(task.held_out.iter_mut())
        .filter(|check| check.run == command)
    {
        check.run = replacement.to_string();
        check.baseline = None;
    }
    let old = task
        .planner_tests
        .sources
        .iter()
        .filter(|source| source.command == command)
        .map(|source| source.source)
        .collect::<Vec<_>>();
    for source in old {
        super::sources::record_source(
            task,
            SourcedCheck {
                command: replacement.to_string(),
                source,
            },
        );
    }
    Ok(())
}

pub(crate) fn append_final(task: &mut Task, commands: &[String], child: &str, now: i64) -> bool {
    let task = &mut task.data;
    let mut changed = false;
    for command in commands {
        let command = command.trim();
        if task
            .verify
            .iter()
            .chain(&task.final_verify)
            .any(|current| current.trim() == command)
        {
            continue;
        }
        task.final_verify.push(command.to_string());
        task.decisions.push(format!(
            "Orchestrator: added {command} to finalVerify for subtask {child}"
        ));
        changed = true;
    }
    if changed {
        task.updated_at = now;
    }
    changed
}

pub(crate) fn append_regression(task: &mut Task, finding: &str, run: &str) {
    let task = &mut task.data;
    task.criteria
        .push(format!("Review finding {finding} is fixed -- check: {run}"));
    task.checks.push(Check {
        criterion: task.criteria.len() - 1,
        run: run.to_string(),
        baseline: Some(Baseline::Fail),
    });
}

/// The file/protection adapter has resolved its IO; adoption may not weaken a
/// command's previously recorded authority.
pub(crate) fn adopt_planner_tests(task: &mut Task, mut tests: PlannerTests) {
    let task = &mut task.data;
    let changed = task.planner_tests.files != tests.files;
    let known = task.planner_tests.sources.clone();
    tests.sources.extend(known);
    task.planner_tests = tests;
    if changed {
        super::baseline::invalidate(task);
    }
    let sources = std::mem::take(&mut task.planner_tests.sources);
    for source in sources {
        super::sources::record_source(task, source);
    }
}

pub(crate) fn replace_planner_files(task: &mut Task, files: Vec<crate::task::PlannedTest>) {
    let task = &mut task.data;
    if task.planner_tests.files != files {
        task.planner_tests.files = files;
        super::baseline::invalidate(task);
    }
}

pub(crate) fn supply_verification(
    task: &mut Task,
    command: String,
    source: crate::task::CheckSource,
) {
    let task = &mut task.data;
    if task.verify.is_empty() {
        task.verify.push(command.clone());
        super::sources::record_source(task, SourcedCheck { command, source });
    }
}

/// Fixes, before any check runs, each grep check whose pattern starts with
/// `-`; one decision line per command. Returns whether anything changed.
pub(crate) fn heal_grep_dashes(task: &mut Task) -> bool {
    let task = &mut task.data;
    let mut changed: Vec<(String, String)> = Vec::new();
    for c in task
        .verify
        .iter_mut()
        .chain(task.final_verify.iter_mut())
        .chain(task.checks.iter_mut().map(|c| &mut c.run))
    {
        if let Some(fixed) = super::commands::fix_grep_dashes(c) {
            if !changed.iter().any(|(old, _)| old == c) {
                changed.push((c.clone(), fixed.clone()));
            }
            *c = fixed;
        }
    }
    for (old, new) in &changed {
        task.decisions.push(format!(
            "Orchestrator: check {old} could not run (grep reads a pattern starting with `-` as an option); rewritten to {new}"
        ));
    }
    !changed.is_empty()
}

impl Task {
    pub(crate) fn forget_planner_commit(&mut self) {
        self.data.planner_tests.commit = None;
    }
    pub(crate) fn record_protected_tests(
        &mut self,
        hashes: std::collections::BTreeMap<String, String>,
    ) {
        self.data.planner_tests.protected = Some(hashes);
    }
    pub(crate) fn record_ruled_test(&mut self, path: String) {
        self.data.planner_tests.ruled.push(path);
    }
}
