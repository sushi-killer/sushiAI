use super::authority::{criterion_commands, removal_refusal, text_check, CheckAuthority, Origin};
use super::snapshot::CriterionEdit;
use crate::task::{CheckSource, SourcedCheck, Task};

#[derive(Debug, PartialEq)]
pub(crate) struct CheckRewrite {
    pub(crate) command: String,
    pub(crate) criterion_text: String,
    pub(crate) reason: String,
}

pub(crate) fn shown_commands(task: &Task, index: usize) -> Vec<String> {
    let mut commands: Vec<String> = task
        .checks
        .iter()
        .filter(|check| check.criterion == index)
        .map(|check| check.run.clone())
        .collect();
    if task
        .held_out
        .as_ref()
        .is_some_and(|check| check.criterion == index)
    {
        commands.push(super::commands::held_out_label(index));
    }
    if let Some(command) = task.criteria.get(index).and_then(|text| text_check(text)) {
        commands.push(command);
    }
    commands.dedup();
    commands
}

pub(crate) fn replace(
    authority: &CheckAuthority,
    task: &mut Task,
    edit: &CriterionEdit,
    rewrite: &CheckRewrite,
) -> Result<(), String> {
    if !edit.snapshot.matches(task) {
        return Err("the criterion edit is stale".to_string());
    }
    let index = edit.index;
    let command = rewrite.command.trim();
    let old_text = task.criteria[index].clone();
    let shown = shown_commands(task, index);
    if command.is_empty() {
        return Err("the new command is empty".to_string());
    }
    if shown.iter().all(|old| old == command) {
        return Err("the new command is the old one".to_string());
    }
    if let Some(why) = removal_refusal(
        authority,
        task,
        &criterion_commands(task, index),
        Origin::Other,
    ) {
        return Err(why);
    }
    let visible: Vec<String> = task
        .checks
        .iter()
        .filter(|check| check.criterion == index)
        .map(|check| check.run.clone())
        .chain(text_check(&old_text))
        .collect();
    let wording = if rewrite.criterion_text.is_empty() {
        &old_text
    } else {
        &rewrite.criterion_text
    };
    let prose = wording
        .split_once("-- check:")
        .map_or(wording.as_str(), |(prose, _)| prose)
        .trim();
    let text = if text_check(&old_text).is_some() {
        format!("{prose} -- check: {command}")
    } else {
        prose.to_string()
    };
    if text.is_empty() {
        return Err("the new criterion text is empty".to_string());
    }
    if text != old_text {
        super::edits::rewrite_criterion(authority, task, index, &text)?;
    }
    replace_references(&mut task.data, index, command, &visible);
    let task = &mut task.data;
    let mut line = format!(
        "Answer judge: criterion {index} check {} rewritten to {command} ({})",
        shown.join(", "),
        rewrite.reason
    );
    if text != old_text {
        line.push_str(&format!("; criterion amended to: {text}"));
    }
    task.decisions.push(line);
    Ok(())
}

fn replace_references(
    task: &mut crate::task::TaskData,
    index: usize,
    command: &str,
    visible: &[String],
) {
    for check in task
        .checks
        .iter_mut()
        .chain(task.held_out.iter_mut())
        .filter(|check| check.criterion == index)
    {
        check.run = command.to_string();
    }
    for current in task
        .verify
        .iter_mut()
        .chain(&mut task.final_verify)
        .filter(|current| visible.contains(current))
    {
        *current = command.to_string();
    }
    let mut kept = false;
    task.checks
        .retain(|check| check.criterion != index || !std::mem::replace(&mut kept, true));
    for list in [&mut task.verify, &mut task.final_verify] {
        let mut first = true;
        list.retain(|current| current != command || std::mem::replace(&mut first, false));
    }
    if !task
        .planner_tests
        .sources
        .iter()
        .any(|source| source.command == command)
    {
        task.planner_tests.sources.push(SourcedCheck {
            command: command.to_string(),
            source: CheckSource::Planner,
        });
    }
    super::baseline::invalidate(task);
}

#[cfg(test)]
mod tests;
