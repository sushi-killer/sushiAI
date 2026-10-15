use crate::clock::now_ms;
use crate::engine::answer_policy::try_policy_answer;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::reply::Reply;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::AnsweredBy;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// [`park_for_answer`] after one shot at the answer policy: a rule or the
/// judge may answer the question before it reaches the owner.
pub(in crate::engine) async fn wait_for_answer(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<String> {
    let id = task.id.clone();
    let task_id = id.as_str();
    if let Some(answer) = try_policy_answer(app, task, cancel).await {
        return Some(answer);
    }
    let answer = park_for_answer(app, task, pending_answer, cancel, permit).await;
    app.take_agent_answer(task_id);
    answer
}

/// Parks on a fresh one-shot channel until either `task.answer` delivers an
/// answer or `cancel` fires. `task` must already have its `question`/
/// `status: Waiting` set by the caller -- it's persisted here, *after* the
/// one-shot is installed, so a `task.answer` that arrives the instant the
/// broadcast reaches a client can never beat the one-shot into existence
/// and get wrongly rejected as "no live parked loop". Persists the
/// decision/status transition either way afterwards, so a caller never has
/// to duplicate that bookkeeping.
pub(in crate::engine) async fn park_for_answer(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<String> {
    if !prepare_park(app, task, cancel, permit) {
        return None;
    }
    match park(app, task, pending_answer, cancel, false).await {
        Parked::Answer(answer) => answer,
        Parked::NoOverseer => None,
    }
}

/// How a park ended.
pub(super) enum Parked {
    Answer(Option<String>),
    /// `need_overseer` was set and the wake neither started a turn nor held
    /// the question: nobody will answer it. The question is still open.
    NoOverseer,
}

pub(super) async fn park(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    need_overseer: bool,
) -> Parked {
    let id = task.id.clone();
    let task_id = id.as_str();
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    app.save_task_logged(task);
    // The wake may hold the question for the overseer or hand it to the
    // owner; the broadcast that shows the waiting task carries either.
    wake_orchestrator_for_waiting(app, task);
    if let Ok(Some(saved)) = app.store.load_task(task_id) {
        if let Some(question) = saved.question.as_ref() {
            task.merge_question_delivery(question);
        }
    }
    if need_overseer && !task.question.as_ref().is_some_and(|q| q.held_for_overseer) {
        *pending_answer.lock().unwrap() = None;
        return Parked::NoOverseer;
    }
    app.broadcast_task(task);
    let reply = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    let evidence = reply.as_ref().and_then(|r| r.default_evidence.clone());
    let like = reply.as_ref().and_then(|r| r.like.clone());
    let result = reply.map(|r| r.text);
    match &result {
        // Keep the caller's copy in sync with either transition so its
        // next save cannot restore the open question.
        Some(answer) => {
            // Peeked: the caller reads it again to decide `by_owner`.
            let caller = app
                .agent_answers
                .lock()
                .unwrap()
                .get(task_id)
                .copied()
                .unwrap_or(AnsweredBy::Owner);
            let question = task.take_question();
            record_answer_for(
                task,
                question.as_ref(),
                &crate::engine::questions::reply::Reply {
                    text: (answer).to_string(),
                    default_evidence: (evidence.as_deref()).map(str::to_string),
                    like: (like.as_deref()).map(str::to_string),
                },
                caller,
            );
            task.resume_phase(TaskStatus::Queued, now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        None => {
            if let Ok(Some(saved)) = app.store.load_task(task_id) {
                *task = saved;
                // The failure ladder ended this task: it stays failed.
                if task.status != TaskStatus::Failed {
                    app.apply_cancellation(task);
                }
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(task);
                }
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                app.advance_queue(&task.repo);
            }
        }
    }
    Parked::Answer(result)
}

pub(super) fn prepare_park(
    app: &App,
    task: &mut Task,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> bool {
    if cancel.is_cancelled() {
        app.apply_cancellation(task);
        task.touch(now_ms());
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        app.advance_queue(&task.repo);
        return false;
    }
    // A task waiting for its owner holds no slot: parallel limits running
    // agents, not open questions.
    permit.take();
    true
}
