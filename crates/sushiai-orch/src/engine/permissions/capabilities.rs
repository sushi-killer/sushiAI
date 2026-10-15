use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::decision::drop_criterion;
use crate::engine::state::App;
use crate::store::permissions::add_rule;
use crate::store::permissions::rules_for;
use crate::store::permissions::Scope;
use crate::task::is_option;
use crate::task::AskedBy;
use crate::task::MissingTool;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;

const OWNER_ACTION: &str = "I will do it: drop the criterion";

const CONTINUE_ANYWAY: &str = "Continue anyway";

pub(in crate::engine) fn missing_tool_question(task: &Task, missing: &MissingTool) -> Question {
    let criterion = task
        .criteria
        .get(missing.criterion)
        .map(String::as_str)
        .unwrap_or("");
    let mut options = Vec::new();
    if let Some(id) = &missing.tool_id {
        options.push(format!("Enable {id} for this task"));
    }
    options.push(OWNER_ACTION.to_string());
    options.push(CONTINUE_ANYWAY.to_string());
    Question::new(
        format!(
            "Criterion {} needs {}, which this task will not have: {criterion}",
            missing.criterion, missing.capability
        ),
        options,
        QuestionKind::Permission,
        AskedBy::Brief,
        crate::clock::now_ms(),
    )
}

/// Applies the answer to the preflight question.
pub(in crate::engine) fn apply_missing_answer(
    app: &App,
    task: &mut Task,
    missing: &MissingTool,
    answer: &str,
) {
    let enable = missing
        .tool_id
        .as_deref()
        .filter(|id| is_option(answer, &format!("Enable {id} for this task")));
    if let Some(id) = enable {
        add_rule(
            &app.data_dir,
            Scope::Task(&task.id),
            &format!("enable:{id}"),
        );
        task.record_decision(format!("Brief check: {id} enabled for this task"));
    } else if is_option(answer, OWNER_ACTION) {
        let text = task
            .criteria
            .get(missing.criterion)
            .cloned()
            .unwrap_or_default();
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        let _ = drop_criterion(&settings, task, missing.criterion, origin);
        task.record_decision(format!(
            "Brief check: \"{text}\" needs {}; it is an owner action, not this task's",
            missing.capability
        ));
    } else {
        task.record_decision(format!(
            "Brief check: the owner let the task run without {}",
            missing.capability
        ));
    }
}

/// `No tool: <capability>`: how an agent reports a criterion it has no tool
/// for (the brief asks for it as the start of the blocked report's question).
pub(in crate::engine) fn missing_tool_claim(question: &str) -> Option<String> {
    let question = question.trim();
    question
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("no tool:"))?;
    let text = question[8..].trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The connected tool the owner could switch on for an agent's
/// `No tool: <capability>` report, when one matches.
pub(in crate::engine) fn tool_to_enable(
    app: &App,
    task: &Task,
    capability: &str,
) -> Option<String> {
    let settings = app.settings.read().unwrap();
    let lower = capability.to_lowercase();
    settings
        .chat_tools
        .iter()
        .find(|c| {
            !c.enabled
                && !rules_for(&app.data_dir, &task.repo, &task.id)
                    .contains(&format!("enable:{}", c.id))
                && (lower.contains(&c.id.to_lowercase()) || lower.contains(&c.label.to_lowercase()))
        })
        .map(|c| c.id.clone())
}

/// The question for an agent's `No tool: <capability>` report; `enable` is
/// the connected tool the owner could switch on, when one matches.
pub(in crate::engine) fn no_tool_question(capability: &str, enable: Option<&str>) -> Question {
    let mut options: Vec<String> = enable
        .map(|id| format!("Enable {id} for this task"))
        .into_iter()
        .collect();
    options.push(OWNER_ACTION.to_string());
    options.push("stop".to_string());
    Question::new(
        format!("The agent has no tool for {capability}; a retry cannot give it one."),
        options,
        QuestionKind::Permission,
        AskedBy::Implement,
        crate::clock::now_ms(),
    )
}

/// Applies the owner's answer to a [`no_tool_question`].
pub(in crate::engine) fn apply_no_tool_answer(
    app: &App,
    task: &mut Task,
    capability: &str,
    answer: &str,
) {
    if let Some(id) = answer
        .trim()
        .strip_prefix("Enable ")
        .and_then(|rest| rest.strip_suffix(" for this task"))
    {
        add_rule(
            &app.data_dir,
            Scope::Task(&task.id),
            &format!("enable:{id}"),
        );
        task.record_decision(format!("Permission: {id} enabled for this task"));
    } else {
        task.record_decision(format!(
            "Orchestrator: the owner will do the part that needs {capability}; do not attempt it, finish the rest and name what is left in your handoff"
        ));
    }
}
