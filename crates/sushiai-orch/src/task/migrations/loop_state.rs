use super::base_state::legacy_base_tail;
use crate::task::{
    AnsweredBy, AttemptStatus, QuestionKind, Stage, TaskData, UnreviewedPart, STATE_VERSION,
};

/// Version 1 of the loop state: derives the typed fields from the decision
/// lines and question texts older versions wrote them into. This is the only
/// reader of those formats; afterwards the fields are the state.
pub(super) fn migrate_loop_state(task: &mut TaskData) {
    if task.loop_state.state_version >= 1 {
        return;
    }
    let decisions = &task.decisions;
    let lines = |prefix: &'static str| decisions.iter().filter(move |d| d.starts_with(prefix));
    let unreviewed_attempt = unreviewed_attempt(task);
    let agent_restarted = task
        .decisions
        .iter()
        .any(|d| d == "Orchestrator: restarted a failed task");
    let dependency_restarted = task
        .decisions
        .iter()
        .any(|d| d == "Orchestrator: restarted a dependency that failed on the environment");
    let approach_changed_after = lines("Ladder: changed approach after attempt ")
        .filter_map(|d| {
            let rest = d.strip_prefix("Ladder: changed approach after attempt ")?;
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .next_back();
    let stuck_retries = lines("Retry later: ").count() as u32;
    // The count belonged to the landing in progress; a landed task has none.
    let rereviews = if task.landed_at.is_some() {
        0
    } else {
        lines("Land: the carry").count() as u32
    };
    let exhausted = "Attempts keep failing";
    let orchestrator = lines("Orchestrator: Attempts keep failing").count();
    let like = lines("Overseer: answered like ")
        .filter(|d| d.contains(&format!(" -- {exhausted} -> ")))
        .count();
    let legacy_lead = lines("Lead: Attempts keep failing -> ").count();
    let lead = task
        .question_history
        .iter()
        .filter(|q| {
            q.kind == QuestionKind::AttemptsFailing
                && q.answered_by == AnsweredBy::Lead
                && !q.answer.trim().eq_ignore_ascii_case("stop")
        })
        .count();
    let non_owner_continues = (orchestrator + like + lead.max(legacy_lead)) as u32;
    let part_suffix = ") landed unreviewed: no review route gave a verdict";
    let unreviewed_parts: Vec<UnreviewedPart> = lines("Orchestrator: subtask \"")
        .filter_map(|d| {
            let rest = d
                .strip_prefix("Orchestrator: subtask \"")?
                .strip_suffix(part_suffix)?;
            let (title, id) = rest.rsplit_once("\" (")?;
            Some(UnreviewedPart {
                id: id.to_string(),
                title: title.to_string(),
            })
        })
        .collect();
    let impossible_answers: Vec<usize> = task
        .assumptions
        .iter()
        .filter(|a| {
            (a.by == "policy" || a.by == "judge") && a.kind == Some(QuestionKind::Impossible)
        })
        .filter_map(|a| legacy_criterion(&a.question))
        .collect();
    migrate_question(task);
    let state = &mut task.loop_state;
    state.state_version = 1;
    state.unreviewed_attempt = unreviewed_attempt;
    state.agent_restarted = agent_restarted;
    state.dependency_restarted = dependency_restarted;
    state.approach_changed_after = approach_changed_after;
    state.stuck_retries = stuck_retries;
    state.rereviews = rereviews;
    state.non_owner_continues = non_owner_continues;
    state.unreviewed_parts = unreviewed_parts;
    state.impossible_answers = impossible_answers;
}

fn unreviewed_attempt(task: &TaskData) -> Option<u32> {
    task
        .attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .filter(|a| {
            task.unreviewed
                && a.status == AttemptStatus::Blocked
                && task.decisions.last().map(String::as_str)
                    == Some(&format!(
                        "Orchestrator: attempt {} committed unreviewed: its open review question was retired",
                        a.n
                    ))
        })
        .map(|a| a.n)
}

fn migrate_question(task: &mut TaskData) {
    if let Some(q) = task.question.as_mut() {
        if q.kind == QuestionKind::Impossible {
            q.criterion = legacy_criterion(&q.text);
            q.evidence = q
                .text
                .split_once(". Evidence: ")
                .map(|(_, e)| e.to_string());
        }
        if q.kind == QuestionKind::AgentQuestion && q.text.starts_with("This task has spent") {
            q.kind = QuestionKind::Budget;
        }
        if q.kind == QuestionKind::PreexistingFailure && q.base_tails.is_empty() {
            q.base_tails = q
                .commands
                .iter()
                .map(|command| legacy_base_tail(&q.text, command))
                .collect();
        }
    }
}

/// Version 3 of the loop state: the commands a parent's final check failed
/// on, which version 2 kept only in a decision line.
pub(super) fn migrate_final_check_failures(task: &mut TaskData) {
    if task.loop_state.state_version >= STATE_VERSION {
        return;
    }
    let mut failures: Vec<String> = Vec::new();
    for d in &task.decisions {
        let command = d
            .strip_prefix("Orchestrator: every subtask landed, but `")
            .and_then(|r| r.split_once("` exited"))
            .map(|(command, _)| command);
        if let Some(command) = command {
            if !failures.iter().any(|c| c == command) {
                failures.push(command.to_string());
            }
        }
    }
    task.loop_state.final_check_failures = failures;
    task.loop_state.state_version = STATE_VERSION;
}

/// The criterion number in the text of an impossible question.
fn legacy_criterion(text: &str) -> Option<usize> {
    text.strip_prefix("Criterion ")?
        .split_once(" cannot be met as written")?
        .0
        .parse()
        .ok()
}
