use super::authority::{criterion_commands, removal_refusal, CheckAuthority, Origin};
use super::snapshot::CriterionEdit;
use crate::task::{AnsweredBy, Task, TaskData};

pub(crate) fn drop_criterion(
    authority: &CheckAuthority,
    task: &mut Task,
    edit: &CriterionEdit,
    origin: Origin,
) -> Result<(), String> {
    let task = &mut task.data;
    if !edit.snapshot.matches(task) {
        return Err("the criterion edit is stale".to_string());
    }
    let n = edit.index;
    if let Some(why) = removal_refusal(authority, task, &criterion_commands(task, n), origin) {
        task.decisions
            .push(format!("Orchestrator: criterion {n} stays: {why}"));
        return Err(why);
    }
    let commands: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for c in criterion_commands(task, n) {
            if !c.is_empty() && !seen.contains(&c) {
                seen.push(c);
            }
        }
        seen
    };
    let text = remove_references(task, n);
    // Who dropped it is on the line: the report lists the criterion as not
    // done, and the owner must not read a judge's drop as their own answer.
    let by = match (origin, task.question_history.last().map(|h| h.answered_by)) {
        (Origin::Owner, _) => "the owner",
        (_, Some(AnsweredBy::Judge)) => "the answer judge",
        (_, Some(AnsweredBy::Policy)) => "the answer policy",
        (_, Some(AnsweredBy::Overseer)) => "the overseer",
        (_, Some(AnsweredBy::Orchestrator)) => "the orchestrator",
        (_, Some(AnsweredBy::Lead)) => "the lead",
        _ => "an agent",
    };
    task.decisions.push(format!(
        "Orchestrator: dropped criterion {n} ({text}) as {by} answered"
    ));
    // The same command elsewhere belongs to the dropped criterion too.
    let mut also: Vec<String> = Vec::new();
    for command in commands {
        if removal_refusal(authority, task, std::slice::from_ref(&command), origin).is_some() {
            continue;
        }
        let mut places: Vec<&str> = Vec::new();
        if task.verify.contains(&command) {
            task.verify.retain(|c| c != &command);
            places.push("verify");
        }
        if task.final_verify.contains(&command) {
            task.final_verify.retain(|c| c != &command);
            places.push("finalVerify");
        }
        if task.checks.iter().any(|c| c.run == command) {
            task.checks.retain(|c| c.run != command);
            places.push("checks");
        }
        if task.held_out.as_ref().is_some_and(|h| h.run == command) {
            task.held_out = None;
            places.push("heldOut");
        }
        if !places.is_empty() {
            also.push(format!("`{command}` from {}", places.join(", ")));
        }
    }
    if !also.is_empty() {
        task.decisions.push(format!(
            "Orchestrator: dropping criterion {n} also removed {}",
            also.join("; ")
        ));
    }
    Ok(())
}

fn remove_references(task: &mut TaskData, n: usize) -> String {
    let text = task.criteria.remove(n);
    task.checks.retain(|c| c.criterion != n);
    if task.held_out.as_ref().is_some_and(|h| h.criterion == n) {
        task.held_out = None;
    }
    for c in task.checks.iter_mut().chain(task.held_out.as_mut()) {
        if c.criterion > n {
            c.criterion -= 1;
        }
    }
    for review in task.attempts.iter_mut().filter_map(|a| a.review.as_mut()) {
        review.criteria.retain(|r| r.index != Some(n));
        for ruling in review.criteria.iter_mut() {
            if let Some(i) = ruling.index.as_mut().filter(|i| **i > n) {
                *i -= 1;
            }
        }
    }
    task.visual_criteria.retain(|visual| visual != &text);
    text
}

pub(crate) fn remove_report_only(
    authority: &CheckAuthority,
    task: &mut Task,
    snapshot: &super::snapshot::CheckSnapshot,
    remove: &[usize],
    origin: Origin,
) -> Result<(), String> {
    let task = &mut task.data;
    if !snapshot.matches(task) {
        return Err("the plan review is stale".to_string());
    }
    for &index in remove {
        if let Some(why) =
            removal_refusal(authority, task, &criterion_commands(task, index), origin)
        {
            return Err(why);
        }
    }
    let mut indexes = remove.to_vec();
    indexes.sort_unstable();
    indexes.dedup();
    for index in indexes.into_iter().rev() {
        if index < task.criteria.len() {
            remove_references(task, index);
        }
    }
    Ok(())
}
