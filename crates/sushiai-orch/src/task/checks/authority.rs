use super::commands::{flat_command, runs_ci_entry};
use crate::task::amendment::Amendment;
use crate::task::checks::types::CheckSource;
use crate::task::lifecycle::PRE_EXISTING_RETRY;
use crate::task::state::{Task, TaskData};

/// Commands whose settings entry requires owner authority. The engine resolves
/// repository paths before constructing this value.
#[derive(Debug, Clone, Default)]
pub(crate) struct CheckAuthority {
    pub named: Vec<String>,
}

impl CheckAuthority {
    pub(crate) fn is_shared(&self, task: &TaskData, command: &str) -> bool {
        let command = flat_command(command);
        if command.is_empty() {
            return false;
        }
        let source = task
            .planner_tests
            .sources
            .iter()
            .filter(|s| flat_command(&s.command) == command)
            .map(|s| s.source)
            .max_by_key(|s| match s {
                CheckSource::Planner => 0,
                CheckSource::Repo => 1,
                CheckSource::Owner => 2,
            })
            .unwrap_or(CheckSource::Repo);
        self.named.iter().any(|c| flat_command(c) == command)
            || source != CheckSource::Planner
            || runs_ci_entry(&command)
    }
}

/// The refusal for a non-owner who tries to drop `command`, a shared check.
fn shared_check_refusal(command: &str) -> String {
    format!(
        "`{command}` is a shared check (not made by the planner for this task, or named by a setting or the owner): only the owner may drop it. An agent can answer \"{}\" or \"stop\", otherwise the owner decides",
        PRE_EXISTING_RETRY
    )
}

/// Who asks for a removal: the owner, or anyone else (the overseer, a chat or
/// MCP client, the answer policy, the judge, a feasibility ruling).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Origin {
    Owner,
    Other,
}

/// THE choke point of check removal: every path that removes or replaces a
/// command in `verify`, `finalVerify`, `checks` or the held-out check (a drop
/// answer, a dropped criterion, an amendment, a feasibility ruling) asks here
/// first. Anyone but the owner is refused when one of `commands` is shared;
/// the caller then keeps the check and escalates to the owner.
pub(crate) fn removal_refusal(
    authority: &CheckAuthority,
    task: &TaskData,
    commands: &[String],
    origin: Origin,
) -> Option<String> {
    if origin == Origin::Owner {
        return None;
    }
    commands
        .iter()
        .find(|c| authority.is_shared(task, c))
        .map(|c| shared_check_refusal(c))
}

/// The commands that go with criterion `n`: its checks, a held-out check on
/// it and the command its `-- check:` part names.
pub(crate) fn criterion_commands(task: &TaskData, n: usize) -> Vec<String> {
    let mut out: Vec<String> = task
        .checks
        .iter()
        .chain(task.held_out.as_ref())
        .filter(|c| c.criterion == n)
        .map(|c| c.run.clone())
        .collect();
    if let Some((_, cmd)) = task.criteria.get(n).and_then(|t| t.split_once("-- check:")) {
        out.push(cmd.trim().to_string());
    }
    out
}

/// The command a criterion's `-- check:` part names.
pub(super) fn text_check(text: &str) -> Option<String> {
    let (_, cmd) = text.split_once("-- check:")?;
    let cmd = cmd.trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// The refusal for a judge replacing criterion `i` with `new_text` when that
/// takes a shared check out of its `-- check:` part.
pub(crate) fn criterion_text_refusal(
    authority: &CheckAuthority,
    task: &TaskData,
    i: usize,
    new_text: &str,
) -> Option<String> {
    let old = task.criteria.get(i).and_then(|t| text_check(t))?;
    if text_check(new_text).as_deref() == Some(old.as_str()) {
        return None;
    }
    removal_refusal(authority, task, &[old], Origin::Other)
}

/// Removes `command` from `verify`, `finalVerify` and `checks`, unless
/// `removal_refusal` says only the owner may.
pub(crate) fn remove_check_command(
    authority: &CheckAuthority,
    task: &mut Task,
    command: &str,
    origin: Origin,
) -> Result<(), String> {
    let task = &mut task.data;
    if let Some(why) = removal_refusal(authority, task, &[command.to_string()], origin) {
        return Err(why);
    }
    task.verify.retain(|c| c != command);
    task.final_verify.retain(|c| c != command);
    task.checks.retain(|c| c.run != command);
    if task.held_out.as_ref().is_some_and(|c| c.run == command) {
        task.held_out = None;
    }
    Ok(())
}

/// The refusal for a non-owner amendment that removes a shared check from
/// `verify`, `finalVerify`, `checks`, the held-out check or a criterion's
/// `-- check:` part: the same rule as dropping one. `task` is the state the
/// amendment lands on (see `effective_task`).
pub(crate) fn shared_check_amend_refusal(
    authority: &CheckAuthority,
    task: &TaskData,
    amendment: &Amendment,
) -> Option<String> {
    let gone = |old: &[String], new: &Option<Vec<String>>| -> Vec<String> {
        new.as_ref().map_or_else(Vec::new, |new| {
            old.iter().filter(|c| !new.contains(c)).cloned().collect()
        })
    };
    let old_checks: Vec<String> = task.checks.iter().map(|c| c.run.clone()).collect();
    let new_checks = amendment
        .checks
        .as_ref()
        .map(|cs| cs.iter().map(|c| c.run.clone()).collect::<Vec<_>>());
    let mut removed = [
        gone(&task.verify, &amendment.verify),
        gone(&task.final_verify, &amendment.final_verify),
        gone(&old_checks, &new_checks),
    ]
    .concat();
    // The held-out check, when the amendment replaces or clears it.
    if let (Some(old), Some(new)) = (&task.held_out, &amendment.held_out) {
        if new.as_ref().is_none_or(|n| n.run != old.run) {
            removed.push(old.run.clone());
        }
    }
    // A check run that a rebuild from a new `-- check:` replaces.
    for (i, command) in amendment.rebuilds(task) {
        removed.extend(
            task.checks
                .iter()
                .filter(|c| c.criterion == i && c.run != command)
                .map(|c| c.run.clone()),
        );
    }
    // A criterion whose `-- check:` command is no longer named by any new
    // criterion, and every criterion that falls off the end.
    if let Some(new) = &amendment.criteria {
        for (n, old) in task.criteria.iter().enumerate() {
            let check_gone = text_check(old).is_some_and(|command| {
                !new.iter()
                    .filter_map(|text| text_check(text))
                    .any(|new| new == command)
            });
            if check_gone || n >= new.len() {
                removed.extend(criterion_commands(task, n));
            }
        }
    }
    removal_refusal(authority, task, &removed, Origin::Other)
}

#[cfg(test)]
mod tests;
