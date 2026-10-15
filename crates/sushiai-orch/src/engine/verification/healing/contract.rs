use crate::engine::answer_policy::check_authority;
use crate::engine::answer_policy::criterion_text_refusal;
use crate::engine::answer_policy::removal_refusal;
use crate::engine::answer_policy::remove_check_command;
use crate::engine::brief_check::check_route_for;
use crate::engine::state::App;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;
use crate::task::checks::authority::Origin;
use crate::task::questions::Assumption;
use crate::task::state::Task;
use crate::verify::healing::reply::Feasibility;

/// The route of the cheap judge runs; `None` when they are off.
pub(in crate::engine) fn judge_route(app: &App, variant: &Variant) -> Option<Route> {
    check_route_for(app, variant)?.1
}

pub(in crate::engine) fn overturnable(
    task: &mut Task,
    question: String,
    answer: String,
    evidence: String,
) {
    task.record_assumption(Assumption {
        question,
        answer,
        evidence,
        by: "orchd".to_string(),
        kind: None,
        attempt: None,
        commands: Vec::new(),
        overturned: false,
        owner_answer: None,
    });
}

/// Replaces criterion `i` with `text`, keeping its visual mark. A judge may
/// not take a shared check out of the criterion's `-- check:` part
/// (`removal_refusal`): then nothing changes and the reason comes back.
pub(in crate::engine) fn amend_criterion(
    settings: &Settings,
    task: &mut Task,
    i: usize,
    text: &str,
) -> Result<(), String> {
    crate::task::checks::edits::rewrite_criterion(&check_authority(settings, task), task, i, text)
}

#[cfg(test)]
pub(in crate::engine) fn amend_criterion_for_test(
    settings: &Settings,
    task: &mut Task,
    i: usize,
    text: &str,
) -> Result<(), String> {
    amend_criterion(settings, task, i, text)
}

/// Applies a ruling about `command`; `false` when it changed nothing.
pub(in crate::engine) fn apply_feasibility(
    settings: &Settings,
    task: &mut Task,
    command: &str,
    ruling: Feasibility,
) -> bool {
    // A ruling is never the owner's: a shared check keeps gating as written
    // and reaches the twice-on-base question.
    if !matches!(ruling, Feasibility::AsWritten) {
        if let Some(why) = removal_refusal(settings, task, &[command.to_string()], Origin::Other) {
            task.record_decision(format!(
                "Orchestrator: check {command} keeps gating as written: {why}"
            ));
            return true;
        }
    }
    match ruling {
        Feasibility::AsWritten => false,
        Feasibility::Rewrite {
            command: new,
            criterion,
            reason,
        } => {
            if new == command {
                return false;
            }
            if let Some((i, text)) = &criterion {
                if let Some(why) = criterion_text_refusal(settings, task, *i, text) {
                    task.record_decision(format!(
                        "Orchestrator: check {command} keeps gating as written: {why}"
                    ));
                    return true;
                }
            }
            let _ = crate::task::checks::edits::rewrite_command(
                &check_authority(settings, task),
                task,
                command,
                &new,
                Origin::Other,
            );
            let mut line = format!(
                "Orchestrator: check {command} cannot run on this repository ({reason}); rewritten to {new}"
            );
            if let Some((i, text)) = criterion {
                line.push_str(&format!("; criterion amended to: {text}"));
                let _ = amend_criterion(settings, task, i, &text);
            }
            task.record_decision(line);
            overturnable(
                task,
                format!("Can the check `{command}` run on this repository?"),
                format!("No: rewritten to `{new}`"),
                reason,
            );
            true
        }
        Feasibility::Unrelated { reason } => {
            let _ = remove_check_command(settings, task, command, Origin::Other);
            if !task.brief_check.non_gating.iter().any(|c| c == command) {
                task.record_non_gating_check(command.to_string());
            }
            task.record_decision(format!(
                "Orchestrator: check {command} fails on the base for a reason unrelated to the task ({reason}); non-gating"
            ));
            overturnable(
                task,
                format!("Should `{command}` gate this task?"),
                "No: it fails on the base for an unrelated reason".to_string(),
                reason,
            );
            true
        }
    }
}
