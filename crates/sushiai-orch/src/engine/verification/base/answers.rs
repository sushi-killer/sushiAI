use crate::engine::lifecycle::base_wait::base_waited_out;
use crate::engine::state::App;
use crate::policy::paths::glob_match;
use crate::task::checks::commands::check_targets;
use crate::task::lifecycle::is_option;
use crate::task::lifecycle::BaseAnswer;
use crate::task::lifecycle::PRE_EXISTING_RETRY;
use crate::task::questions::AnsweredBy;
use crate::task::state::Task;
use crate::verify::repository::targets_absent_on_base;

/// Options of the pre-existing final-check question, in the order shown.
pub(in crate::engine) const PRE_EXISTING_DROP: &str = "drop all";

pub(in crate::engine) const PRE_EXISTING_OPTIONS: [&str; 3] =
    [PRE_EXISTING_RETRY, PRE_EXISTING_DROP, "stop"];

/// Options of the question about a parent's checks that fail on the base:
/// the subtasks are usually meant to make them pass.
pub(in crate::engine) const KEEP_ALL: &str = "keep all";

pub(in crate::engine) const PARENT_CHECK_OPTIONS: [&str; 3] = [KEEP_ALL, PRE_EXISTING_DROP, "stop"];

pub(in crate::engine) const KEPT_BY_TASK_TARGET: &str = "the task is meant to make it pass";

/// Why a failing check is kept by rule: the files it names do not exist on
/// the base, and the plan creates them.
pub(in crate::engine) const KEPT_BY_PLAN: &str = "targets absent on base; the plan creates them";

/// Keeps a check although it fails on the base, once: the typed state is what
/// stops a relaunch from asking again, the decision line is the owner's log.
pub(in crate::engine) fn keep_check(task: &mut Task, command: &str, reason: &str) {
    if is_kept(task, command) {
        return;
    }
    task.record_kept_check(command.to_string());
    task.record_decision(format!("Orchestrator: kept check {command}, {reason}"));
}

/// Whether `task` already keeps `command` although it fails on the base.
pub(in crate::engine) fn is_kept(task: &Task, command: &str) -> bool {
    task.loop_state.kept_checks.iter().any(|c| c == command)
}

/// Whether the request, goal or criteria name `command` itself: the explicit
/// evidence that the task is meant to make a failing check pass.
pub(in crate::engine) fn command_named_by_task(task: &Task, command: &str) -> bool {
    let flat = crate::task::checks::commands::flat_command(command);
    let named_in_request_or_goal = task
        .request
        .iter()
        .chain(std::iter::once(&task.goal))
        .any(|text| crate::task::checks::commands::flat_command(text).contains(&flat));
    let named_in_criteria = task.criteria.iter().any(|criterion| {
        let flat_criterion = crate::task::checks::commands::flat_command(criterion);
        if !flat_criterion.contains(&flat) {
            return false;
        }
        let Some((prose, gate)) = criterion.split_once("-- check:") else {
            return true;
        };
        let named_in_prose = crate::task::checks::commands::flat_command(prose).contains(&flat);
        let named_only_as_ci_gate = crate::task::checks::commands::runs_ci_entry(command)
            && crate::task::checks::commands::flat_command(gate).contains(&flat)
            && !named_in_prose;
        !named_only_as_ci_gate
    });
    (named_in_request_or_goal || named_in_criteria)
        && !crate::task::checks::commands::runs_ci_entry(command)
}

/// Whether a pre-existing failing check is part of this task's target: the
/// task names the command, or the command names a file the task changed.
/// Changed files are relative to the task's base SHA.
pub(in crate::engine) fn is_task_target(
    task: &Task,
    command: &str,
    changed_files: &[String],
) -> bool {
    command_named_by_task(task, command)
        || check_targets(command).iter().any(|path| {
            changed_files.iter().any(|changed| {
                let changed = changed.strip_prefix("./").unwrap_or(changed);
                changed == path
                    || changed.ends_with(&format!("/{path}"))
                    || path.ends_with(&format!("/{changed}"))
            })
        })
}

/// Whether `target` lies under `path` or matches it: `path` is a file, a
/// directory or a glob of the plan.
fn target_in_plan_path(target: &str, path: &str) -> bool {
    let path = path.strip_prefix("./").unwrap_or(path);
    let dir = format!("{}/", path.trim_end_matches('/'));
    target == path
        || target.starts_with(&dir)
        || glob_match(path, target)
        || glob_match(target, path)
}

/// The failing checks the plan is meant to turn green, kept by rule: a
/// command is kept when at least one of its targets is absent on the base
/// and the task has subtasks, or a target lies under the paths of the task
/// or of a subtask. One decision per kept check is written (once). This is
/// no answer: the answer policy's once-flag and cap do not apply, and no
/// check is removed.
pub(in crate::engine) fn keep_planned_checks(
    app: &App,
    task: &mut Task,
    failing: &[String],
) -> Vec<String> {
    if failing.is_empty() {
        return Vec::new();
    }
    let children: Vec<Task> = app
        .store
        .list_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        .collect();
    let paths: Vec<String> = task
        .paths
        .iter()
        .chain(children.iter().flat_map(|c| c.paths.iter()))
        .cloned()
        .collect();
    let mut kept = Vec::new();
    for command in failing {
        if targets_absent_on_base(&task.repo, &task.base_sha, command).is_empty() {
            continue;
        }
        let planned = !children.is_empty()
            || check_targets(command)
                .iter()
                .any(|t| paths.iter().any(|p| target_in_plan_path(t, p)));
        if !planned {
            continue;
        }
        keep_check(task, command, KEPT_BY_PLAN);
        kept.push(command.clone());
    }
    kept
}

/// The decision line recording an answer to a base-check question, by `who`
/// (`Owner`, `Lead` or `Orchestrator`). A log line: nothing reads it back.
pub(in crate::engine) fn base_check_line(
    who: &str,
    command: &str,
    base_sha: &str,
    answer: &str,
) -> String {
    format!(
        "{who}: base check {command} on {} -> {answer}",
        short7(base_sha)
    )
}

/// The first seven characters of a base SHA, which is all a question quotes.
pub(in crate::engine) fn short7(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Whether the owner, lead or orchestrator agent already answered the
/// base-check question for this command on this base.
pub(in crate::engine) fn base_check_answered(task: &Task, command: &str, base_sha: &str) -> bool {
    let sha = short7(base_sha);
    task.loop_state.base_answers.iter().any(|a| {
        a.command == command
            && short7(&a.sha) == sha
            && matches!(a.by.as_str(), "Owner" | "Orchestrator" | "Lead")
    })
}

/// Whether a non-owner already retried this command. The RPC gate passes a
/// base SHA; the answer policy checks the command across bases, as its rules do.
pub(in crate::engine) fn non_owner_base_retry(
    task: &Task,
    command: &str,
    base_sha: Option<&str>,
) -> bool {
    let sha = base_sha.map(short7);
    task.loop_state.base_answers.iter().any(|a| {
        a.retry
            && a.command == command
            && matches!(a.by.as_str(), "Orchestrator" | "Lead" | "policy" | "judge")
            && sha.as_ref().is_none_or(|sha| short7(&a.sha) == *sha)
    })
}

fn base_retry_line(command: &str, base_sha: &str) -> String {
    format!("Orchestrator: retry base check {command} after {base_sha} moves")
}

/// Records an answer to a base-check question, by `by` (`Owner`, `Lead`,
/// `Orchestrator`, `policy` or `judge`): the typed answer per command is the
/// state; a retry also leaves its decision line for the owner to read.
pub(in crate::engine) fn record_base_answer(
    task: &mut Task,
    commands: &[String],
    base_sha: &str,
    by: &str,
    answer: &str,
) {
    let retry = is_option(answer, PRE_EXISTING_RETRY);
    for command in commands {
        let entry = BaseAnswer {
            command: command.clone(),
            sha: base_sha.to_string(),
            by: by.to_string(),
            retry,
        };
        if task.loop_state.base_answers.contains(&entry) {
            continue;
        }
        task.record_base_answer(entry);
        if retry {
            task.record_decision(base_retry_line(command, base_sha));
        }
    }
}

/// A retry remains pending until its exact base moves, including after a
/// restart and when the answer came from the policy.
pub(in crate::engine) fn base_retry_requested(
    task: &Task,
    commands: &[String],
    base_sha: &str,
) -> bool {
    // A wait that ran out its bound is not asked for again on the same base.
    if base_waited_out(task, base_sha) {
        return false;
    }
    commands.iter().any(|command| {
        task.loop_state.base_answers.iter().any(|a| {
            // An old record may quote only the first seven characters.
            a.retry
                && &a.command == command
                && (a.sha == base_sha || (a.sha.len() <= 7 && base_sha.starts_with(&a.sha)))
        })
    })
}

/// Records an answer to a base-check question about `commands`, asked on
/// `base_sha`, one line per command, unless the answer policy gave it (`policy_answered`). The line
/// says who answered, read from the question history entry the wait just
/// wrote: `Owner:`, `Lead:` or `Orchestrator:`.
pub(in crate::engine) fn record_owner_base_check(
    task: &mut Task,
    commands: &[String],
    base_sha: &str,
    answer: &str,
    policy_answered: bool,
) {
    let by = answerer_name(task.question_history.last().map(|h| h.answered_by));
    record_base_answer(task, commands, base_sha, by, answer);
    if !policy_answered {
        if let Some(who) = answerer_label(task) {
            for command in commands {
                task.record_decision(base_check_line(who, command, base_sha, answer));
            }
        }
    }
}

/// Who gave an answer, as the typed state names them.
pub(in crate::engine) fn answerer_name(by: Option<AnsweredBy>) -> &'static str {
    match by {
        Some(AnsweredBy::Orchestrator | AnsweredBy::Overseer) => "Orchestrator",
        Some(AnsweredBy::Lead) => "Lead",
        Some(AnsweredBy::Policy) => "policy",
        Some(AnsweredBy::Judge) => "judge",
        Some(AnsweredBy::Owner) | None => "Owner",
    }
}

/// Who gave the answer the wait just recorded: `Owner` (also when nothing
/// was recorded), `Lead` or `Orchestrator` for an agent, `None` for a rule or judge,
/// whose own line already says so.
pub(in crate::engine) fn answerer_label(task: &Task) -> Option<&'static str> {
    match task.question_history.last().map(|h| h.answered_by) {
        Some(AnsweredBy::Orchestrator | AnsweredBy::Overseer) => Some("Orchestrator"),
        Some(AnsweredBy::Lead) => Some("Lead"),
        Some(AnsweredBy::Policy | AnsweredBy::Judge) => None,
        Some(AnsweredBy::Owner) | None => Some("Owner"),
    }
}

#[cfg(test)]
mod tests;
