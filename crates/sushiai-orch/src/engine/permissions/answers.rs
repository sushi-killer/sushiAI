use crate::engine::permissions::hook::denied_reason;
use crate::engine::state::App;
use crate::policy::permissions::answer_line;
use crate::policy::permissions::parse_choice;
use crate::policy::permissions::question_text;
use crate::policy::permissions::Choice;
use crate::policy::permissions::Subject;
use crate::policy::permissions::OPTIONS;
use crate::store::permissions::add_rule;
use crate::store::permissions::Scope;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;

pub(in crate::engine) fn permission_question(subjects: &[Subject], detail: &str) -> Question {
    let mut text = question_text(subjects);
    if !detail.is_empty() {
        text.push('\n');
        text.push_str(detail);
    }
    Question::new(
        text,
        OPTIONS.iter().map(|o| o.to_string()).collect(),
        QuestionKind::Permission,
        AskedBy::Implement,
        crate::clock::now_ms(),
    )
}

/// Whether every subject already has an answer recorded on the task: the
/// same cause again after the owner has spoken.
pub(in crate::engine) fn already_answered(task: &Task, subjects: &[Subject]) -> bool {
    subjects.iter().all(|s| {
        task.loop_state
            .answered_permissions
            .iter()
            .any(|name| name == s.name())
    })
}

/// Notes the owner's answer for `subject`: the typed name is the state, the
/// line is the owner's log.
pub(in crate::engine) fn record_permission_answer(
    task: &mut Task,
    subject: &Subject,
    choice: &Choice,
) {
    let name = subject.name().to_string();
    task.remember_permission(name);
    task.record_decision(answer_line(subject, choice));
}

/// Applies the owner's answer to the subjects a finished run was refused.
pub(in crate::engine) fn apply_answer(
    app: &App,
    task: &mut Task,
    subjects: &[Subject],
    answer: &str,
) {
    let choice = parse_choice(answer);
    for subject in subjects {
        record_permission_answer(task, subject, &choice);
        match &choice {
            Choice::Once => add_rule(&app.data_dir, Scope::Task(&task.id), &subject.rule()),
            Choice::Always => add_rule(&app.data_dir, Scope::Repo(&task.repo), &subject.rule()),
            Choice::Deny(_) => {
                task.record_decision(format!("Orchestrator: {}", denied_reason(subject, None)))
            }
        }
    }
}
