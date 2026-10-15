use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::attempts::report::{parse_impossible, Impossible, Outcome, Report};
use crate::engine::decision::drop_criterion;
use crate::engine::decision::{
    advance_after_failure_with, impossible_drop_target, impossible_question, record_failure,
};
use crate::engine::ladder;
use crate::engine::lifecycle::report::ImplementerReport;
use crate::engine::permissions::capabilities::{
    apply_no_tool_answer, missing_tool_claim, no_tool_question, tool_to_enable,
};
use crate::engine::persistence::contract::ContractInput;
use crate::engine::questions::exhausted::{settle_exhausted_answer, wait_for_exhausted_answer};
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::task::{AskedBy, FailureKind, Question, QuestionKind, Task, TaskStatus};
use std::sync::Arc;

pub(in crate::engine) enum Action {
    Proceed,
    Retry(u32),
    Stop,
}

pub(in crate::engine) async fn resolve(
    app: &Arc<App>,
    task: &mut Task,
    report: &ImplementerReport,
    budget: u32,
    slot: &mut QuestionSlot<'_>,
) -> Action {
    let idx = task.attempts.len() - 1;
    if let Some(claim) = parse_impossible(&report.final_text, task.criteria.len()) {
        return impossible(app, task, &claim, slot).await;
    }
    let Some(report) = report
        .report
        .as_ref()
        .filter(|report| report.outcome == Outcome::Blocked)
    else {
        return Action::Proceed;
    };
    record_failure(task, idx, FailureKind::Blocked, report.question.clone());
    let base = ladder::base_tails(app, &task.base_sha);
    if !advance_after_failure_with(task, budget, &base) {
        return match wait_for_exhausted_answer(app, task, slot).await {
            Ok(answered) => {
                settle_exhausted_answer(app, task, idx, &answered.text);
                Action::Retry(2)
            }
            Err(WaitEnd::Changed) => Action::Retry(0),
            Err(WaitEnd::Stopped) => Action::Stop,
        };
    }
    if let Some(capability) = missing_tool_claim(&report.question) {
        return missing_tool(app, task, &capability, slot).await;
    }
    ask_agent(app, task, report, slot).await
}

async fn impossible(
    app: &Arc<App>,
    task: &mut Task,
    claim: &Impossible,
    slot: &mut QuestionSlot<'_>,
) -> Action {
    let idx = task.attempts.len() - 1;
    let question = impossible_question(task, claim);
    let admission = ContractInput::capture(task);
    record_failure(task, idx, FailureKind::Blocked, question.text.clone());
    task.mark_attempt_blocked(idx);
    let publication = crate::task::attempts::inspection::BlockedInspection::capture(task, idx)
        .and_then(|inspection| {
            admission.publish(app, task, slot.cancel(), |fresh| inspection.apply(fresh))
        });
    match publication {
        Ok(publication) => match publication.into_result() {
            Ok(()) => {}
            Err(WaitEnd::Changed) => return Action::Retry(0),
            Err(WaitEnd::Stopped) => return Action::Stop,
        },
        Err(error) => {
            eprintln!(
                "orch: publishing blocked attempt {} failed: {error}",
                task.id
            );
            return Action::Stop;
        }
    }
    task.ask(question.clone(), task.updated_at);
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    let answer = match slot.answer(app, task).await {
        Ok(answer) => answer,
        Err(WaitEnd::Changed) => return Action::Retry(0),
        Err(WaitEnd::Stopped) => return Action::Stop,
    };
    if let Some(index) = impossible_drop_target(&question, &answer) {
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        let _ = drop_criterion(&settings, task, index, origin);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    Action::Retry(0)
}

async fn missing_tool(
    app: &Arc<App>,
    task: &mut Task,
    capability: &str,
    slot: &mut QuestionSlot<'_>,
) -> Action {
    let enable = tool_to_enable(app, task, capability);
    task.ask(
        no_tool_question(capability, enable.as_deref()),
        task.updated_at,
    );
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    let answer = match slot.answer(app, task).await {
        Ok(answer) => answer,
        Err(WaitEnd::Changed) => return Action::Retry(0),
        Err(WaitEnd::Stopped) => return Action::Stop,
    };
    apply_no_tool_answer(app, task, capability, &answer);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    Action::Retry(0)
}

async fn ask_agent(
    app: &Arc<App>,
    task: &mut Task,
    report: &Report,
    slot: &mut QuestionSlot<'_>,
) -> Action {
    let idx = task.attempts.len() - 1;
    let mut question = Question::new(
        report.question.clone(),
        vec![],
        QuestionKind::AgentQuestion,
        AskedBy::Implement,
        now_ms(),
    );
    question.failure_signature = task.attempts[idx]
        .failure
        .as_ref()
        .map(|failure| failure.signature.clone());
    question.risk = report.risk.clone();
    task.ask(question, task.updated_at);
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    match slot.answer(app, task).await {
        Ok(_) | Err(WaitEnd::Changed) => Action::Retry(0),
        Err(WaitEnd::Stopped) => Action::Stop,
    }
}
