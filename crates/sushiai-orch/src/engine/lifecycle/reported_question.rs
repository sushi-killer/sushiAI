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
use crate::engine::questions::exhausted::{settle_exhausted_answer, wait_for_exhausted_answer};
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
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
    idx: usize,
    report: &ImplementerReport,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
    budget: u32,
) -> Action {
    if let Some(claim) = parse_impossible(&report.final_text, task.criteria.len()) {
        return impossible(app, task, idx, &claim, signals, permit).await;
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
        return match wait_for_exhausted_answer(
            app,
            task,
            &signals.pending_answer,
            &signals.cancel,
            permit,
        )
        .await
        {
            Some(answered) => {
                settle_exhausted_answer(app, task, idx, &answered.text);
                Action::Retry(2)
            }
            None => Action::Stop,
        };
    }
    if let Some(capability) = missing_tool_claim(&report.question) {
        return missing_tool(app, task, &capability, signals, permit).await;
    }
    ask_agent(app, task, idx, report, signals, permit).await
}

async fn impossible(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    claim: &Impossible,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Action {
    let question = impossible_question(task, claim);
    record_failure(task, idx, FailureKind::Blocked, question.text.clone());
    task.mark_attempt_blocked(idx);
    task.ask(question.clone(), task.updated_at);
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    let Some(answer) =
        wait_for_answer(app, task, &signals.pending_answer, &signals.cancel, permit).await
    else {
        return Action::Stop;
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
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Action {
    let enable = tool_to_enable(app, task, capability);
    task.ask(
        no_tool_question(capability, enable.as_deref()),
        task.updated_at,
    );
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    let Some(answer) =
        wait_for_answer(app, task, &signals.pending_answer, &signals.cancel, permit).await
    else {
        return Action::Stop;
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
    idx: usize,
    report: &Report,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Action {
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
    match wait_for_answer(app, task, &signals.pending_answer, &signals.cancel, permit).await {
        Some(_) => Action::Retry(0),
        None => Action::Stop,
    }
}
