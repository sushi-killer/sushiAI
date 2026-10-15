use super::snapshot::CheckSnapshot;
use crate::task::{Baseline, Task, TaskData};

pub(crate) type BaselineSlot = (Option<usize>, usize, String);

#[derive(Debug, Clone)]
pub(crate) struct BaselineTicket {
    snapshot: CheckSnapshot,
    pub(crate) todo: Vec<BaselineSlot>,
}

impl BaselineTicket {
    pub(crate) fn capture(task: &TaskData) -> Self {
        Self {
            snapshot: CheckSnapshot::capture(task),
            todo: baseline_todo(task),
        }
    }
}

pub(crate) fn baseline_todo(task: &TaskData) -> Vec<BaselineSlot> {
    let mut todo: Vec<_> = task
        .checks
        .iter()
        .enumerate()
        .filter(|(_, check)| check.baseline.is_none())
        .map(|(index, check)| (Some(index), check.criterion, check.run.clone()))
        .collect();
    if let Some(check) = task
        .held_out
        .as_ref()
        .filter(|check| task.variant().held_out && check.baseline.is_none())
    {
        todo.push((None, check.criterion, check.run.clone()));
    }
    todo
}

pub(crate) struct BaselineObservation {
    pub(crate) baseline: Baseline,
    pub(crate) code: Option<i32>,
}

pub(crate) fn record(
    task: &mut Task,
    ticket: &BaselineTicket,
    results: &[BaselineObservation],
    now: i64,
) -> bool {
    let task = &mut task.data;
    if results.len() != ticket.todo.len() || !ticket.snapshot.matches(task) {
        return false;
    }
    for ((slot, criterion, command), result) in ticket.todo.iter().zip(results) {
        let shown = slot.map_or_else(
            || format!("held-out check (criterion {criterion})"),
            |_| command.clone(),
        );
        match result.baseline {
            Baseline::Pass => task.decisions.push(format!(
                "Orchestrator: check for criterion {criterion} already passes on base, not grounded: {shown}"
            )),
            Baseline::Env => task.decisions.push(format!(
                "Orchestrator: check for criterion {criterion} could not run on base (exit {}), left out: {shown}",
                result.code.map(|code| code.to_string()).unwrap_or_else(|| "none".to_string())
            )),
            Baseline::Fail => {}
        }
        let check = match slot {
            Some(index) => &mut task.checks[*index],
            None => task.held_out.as_mut().expect("ticket matched"),
        };
        check.baseline = Some(result.baseline);
    }
    task.updated_at = now;
    true
}

/// Adopts completed observations only for the identical check plan and base.
pub(crate) fn adopt(task: &mut Task, completed: &TaskData) -> bool {
    let task = &mut task.data;
    if !CheckSnapshot::capture(completed).matches(task) {
        return false;
    }
    for (check, observed) in task
        .checks
        .iter_mut()
        .chain(task.held_out.iter_mut())
        .zip(completed.checks.iter().chain(completed.held_out.iter()))
    {
        if observed.baseline.is_some() {
            check.baseline = observed.baseline;
        }
    }
    true
}

pub(crate) fn invalidate(task: &mut TaskData) {
    for check in task.checks.iter_mut().chain(task.held_out.iter_mut()) {
        check.baseline = None;
    }
    task.brief_check.feasibility_done = false;
}
