use crate::engine::answer_policy::record_answered_question;
use crate::engine::answer_policy::shared_check_drop_refusal;
use crate::engine::answer_policy::{record_policy_answer, AutomaticAnswer};
use crate::engine::decision::EXHAUSTED_QUESTION;
use crate::engine::lines::orchestrator_answer_line;
use crate::engine::lines::overseer_like_line;
use crate::engine::lines::policy_answer_line;
use crate::engine::questions::reply::Reply;
use crate::engine::state::App;
use crate::engine::verification::base::answers::non_owner_base_retry;
use crate::policy::questions::is_recheck;
use crate::policy::questions::is_refusal;
use crate::policy::questions::limits::hard_limit;
use crate::policy::questions::limits::HardLimit;
use crate::policy::questions::owner_only;
use crate::policy::settings::Settings;
use crate::task::is_option;
use crate::task::AnsweredBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::PRE_EXISTING_RETRY;

/// The question text an orchestrator answer line carries. An "Attempts keep
/// failing" question always uses the fixed text the continue cap counts by,
/// not its own text, which names the failure.
pub(in crate::engine) fn answer_line_question(q: &Question) -> &str {
    if q.kind == QuestionKind::AttemptsFailing {
        EXHAUSTED_QUESTION
    } else {
        &q.text
    }
}

/// Why `answer` to the waiting `question` of `task` is refused for a caller
/// that is not the owner: the gate that refuses it, with the reason the caller
/// is told. Owner-only kinds keep only refusals, a shared check is not dropped
/// by an agent, and the overseer and the answer policy stay under
/// the hard limits (`decision_memory.rs`); a lead or chat caller relays what the owner said,
/// so the hard limits do not bind it.
pub(in crate::engine) fn non_owner_refusal(
    settings: &Settings,
    task: &Task,
    question: &Question,
    answer: &str,
    caller: AnsweredBy,
) -> Option<String> {
    if owner_only(question.kind) && !is_refusal(question, answer) && !is_recheck(question, answer) {
        return Some(format!(
            "this {:?} question is the owner's to allow: an agent can only stop or deny it",
            question.kind
        ));
    }
    if let Some(why) = shared_check_drop_refusal(settings, task, question, answer) {
        return Some(why);
    }
    if question.kind == QuestionKind::PreexistingFailure
        && !question.commands.is_empty()
        && is_option(answer, PRE_EXISTING_RETRY)
        && question
            .commands
            .iter()
            .all(|command| non_owner_base_retry(task, command, Some(&task.base_sha)))
    {
        return Some(
            "these checks already got a non-owner retry on this base: the owner decides now"
                .to_string(),
        );
    }
    if caller == AnsweredBy::Overseer && question.kind == QuestionKind::AttemptsFailing {
        let continues = answer.trim().to_ascii_lowercase().starts_with("continue");
        if !continues && !is_option(answer, "change approach") {
            return Some(
                "the overseer may only continue or change approach on attempts keep failing: the failure ladder ends the task otherwise"
                    .to_string(),
            );
        }
    }
    if matches!(caller, AnsweredBy::Overseer | AnsweredBy::Policy) && !owner_only(question.kind) {
        if let Some(limit) =
            hard_limit(question, answer).filter(|l| *l != HardLimit::SharedCheckDrop)
        {
            return Some(format!(
                "this question is under a hard limit ({limit:?}): the owner decides, not an agent"
            ));
        }
    }
    None
}

/// The options of the waiting `question` an overseer may answer with: the
/// ones [`non_owner_refusal`] lets through. Free text meets the same gates
/// when it is sent.
pub(super) fn overseer_allowed_answers(
    settings: &Settings,
    task: &Task,
    question: &Question,
) -> Vec<String> {
    question
        .options
        .iter()
        .filter(|option| {
            non_owner_refusal(settings, task, question, option, AnsweredBy::Overseer).is_none()
        })
        .cloned()
        .collect()
}

/// Records a parked question's answer and its caller.
#[cfg(test)]
pub(in crate::engine) fn record_answer(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    caller: AnsweredBy,
) {
    record_answer_like(task, question, answer, caller, None);
}

/// [`record_answer`], where an overseer answer may name what it was given like.
pub(super) fn record_answer_like(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    caller: AnsweredBy,
    like: Option<&str>,
) {
    if question.is_some_and(|q| q.kind == QuestionKind::AttemptsFailing) {
        task.reset_rate_pauses();
    }
    let text = question.map(answer_line_question).unwrap_or_default();
    let line = match caller {
        AnsweredBy::Owner => format!("Owner: {answer}"),
        AnsweredBy::Lead => format!("Lead: {answer}"),
        AnsweredBy::Overseer => match like {
            Some(like) => overseer_like_line(like, text, answer),
            None => orchestrator_answer_line(text, answer, "answered by the overseer"),
        },
        AnsweredBy::Orchestrator => {
            orchestrator_answer_line(text, answer, "answered by the orchestrator chat")
        }
        AnsweredBy::Policy => policy_answer_line(text, answer, "policy", ""),
        AnsweredBy::Judge => policy_answer_line(text, answer, "judge", ""),
    };
    task.record_decision(line);
    if let Some(q) = question {
        record_answered_question(task, q, answer, caller);
    }
}

/// [`record_answer`], or a policy assumption when the waiting SLA gave this
/// answer (`wait_sla.rs`): a default the owner can overturn.
pub(in crate::engine) fn record_answer_for(
    task: &mut Task,
    question: Option<&Question>,
    reply: &Reply,
    caller: AnsweredBy,
) {
    match (question, reply.default_evidence.as_deref()) {
        (Some(q), Some(evidence)) => {
            let attempt_n = task.attempts.last().map(|a| a.n).unwrap_or(0);
            record_policy_answer(
                task,
                q,
                &AutomaticAnswer::Policy {
                    text: reply.text.to_string(),
                    evidence: evidence.to_string(),
                },
                attempt_n,
            );
        }
        _ => record_answer_like(task, question, &reply.text, caller, reply.like.as_deref()),
    }
}

impl App {
    /// Takes the non-owner caller of the answer that just arrived.
    pub(in crate::engine) fn take_agent_answer(&self, task_id: &str) -> Option<AnsweredBy> {
        self.agent_answers.lock().unwrap().remove(task_id)
    }
}
