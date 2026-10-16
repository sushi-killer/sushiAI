use crate::clock::now_ms;
use crate::engine::answer_policy::publication::PolicyResult;
use crate::engine::answer_policy::try_policy_answer;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::{AnsweredBy, Task, TaskStatus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

pub(in crate::engine) struct QuestionSlot<'a> {
    signals: &'a TaskSignals,
    permit: &'a mut Option<SlotPermit>,
}

impl<'a> QuestionSlot<'a> {
    pub(in crate::engine) fn new(
        signals: &'a TaskSignals,
        permit: &'a mut Option<SlotPermit>,
    ) -> Self {
        Self { signals, permit }
    }

    pub(in crate::engine) fn cancel(&self) -> &CancelToken {
        &self.signals.cancel
    }

    pub(in crate::engine) async fn answer(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
    ) -> Result<String, WaitEnd> {
        let task_id = task.id.clone();
        match try_policy_answer(app, task, self.cancel()).await {
            PolicyResult::Answered(answer) => return Ok(answer),
            PolicyResult::Stale => return Err(WaitEnd::Changed),
            PolicyResult::Stopped => return Err(WaitEnd::Stopped),
            PolicyResult::Open => {}
        }
        let answer = self.park(app, task).await;
        app.take_agent_answer(&task_id);
        answer.ok_or(WaitEnd::Stopped)
    }

    pub(in crate::engine) async fn park(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
    ) -> Option<String> {
        if !self.prepare(app, task) {
            return None;
        }
        match self.park_with(app, task, false).await {
            Parked::Answer(answer) => answer,
            Parked::NoOverseer => None,
        }
    }

    pub(in crate::engine) async fn resume(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        status: TaskStatus,
    ) -> bool {
        task.resume_phase(status, now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        if self.permit.is_none() {
            let acquired = tokio::select! {
                _ = self.signals.cancel.cancelled() => None,
                p = app.slots.clone().acquire_owned() => p.ok(),
            };
            match acquired {
                Some(p) => *self.permit = Some(p),
                None => {
                    mark_stopped_if_not_already(app, &task.id).await;
                    return false;
                }
            }
        }
        true
    }

    pub(in crate::engine) fn release(&mut self) {
        self.permit.take();
    }

    pub(in crate::engine) async fn pause(&mut self, duration: Duration) -> bool {
        self.release();
        tokio::select! {
            _ = self.signals.cancel.cancelled() => false,
            _ = tokio::time::sleep(duration) => true,
        }
    }

    pub(super) fn prepare(&mut self, app: &App, task: &mut Task) -> bool {
        if self.signals.cancel.is_cancelled() {
            app.apply_cancellation(task);
            task.touch(now_ms());
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
            app.advance_queue(&task.repo);
            return false;
        }
        self.release();
        true
    }

    pub(super) async fn park_with(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        need_overseer: bool,
    ) -> Parked {
        let id = task.id.clone();
        let task_id = id.as_str();
        let (tx, rx) = oneshot::channel();
        *self.signals.pending_answer.lock().unwrap() = Some(tx);
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
            *self.signals.pending_answer.lock().unwrap() = None;
            return Parked::NoOverseer;
        }
        app.broadcast_task(task);
        let reply = tokio::select! {
            _ = self.signals.cancel.cancelled() => None,
            answer = rx => answer.ok(),
        };
        *self.signals.pending_answer.lock().unwrap() = None;
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
}

pub(super) enum Parked {
    Answer(Option<String>),
    NoOverseer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum WaitEnd {
    Changed,
    Stopped,
}
