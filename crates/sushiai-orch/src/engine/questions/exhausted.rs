use crate::clock::now_ms;
use crate::engine::answer_policy::try_policy_answer;
use crate::engine::ladder;
use crate::engine::questions::parking::park;
use crate::engine::questions::parking::prepare_park;
use crate::engine::questions::parking::Parked;
use crate::engine::questions::reply::Reply;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::is_option;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// How many times non-owner sessions may extend a task's budget on their own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
pub(in crate::engine) const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

/// The "attempts keep failing" question: the overseer decides it (it sees the
/// last failure, e.g. the review findings, and can say what to fix or that a
/// finding is wrong), up to [`MAX_ORCHESTRATOR_CONTINUES`] times per task;
/// after that, or with auto-answer off, the failure ladder takes one changed
/// approach and then fails the task (`ladder.rs`): the owner is never asked by
/// the ladder. The answer policy goes first: it continues once on its own.
pub(in crate::engine) async fn wait_for_exhausted_answer(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<Answered> {
    let id = task.id.clone();
    let task_id = id.as_str();
    if task.queue.park.is_some() || task.status == TaskStatus::Failed {
        task.touch(now_ms());
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        app.advance_queue(&task.repo);
        return None;
    }
    if let Some(text) = try_policy_answer(app, task, cancel).await {
        return Some(Answered { text });
    }
    let auto_answer = app.settings.read().unwrap().auto_answer;
    if (task.loop_state.non_owner_continues as usize) < MAX_ORCHESTRATOR_CONTINUES && auto_answer {
        if !prepare_park(app, task, cancel, permit) {
            return None;
        }
        match park(app, task, pending_answer, cancel, true).await {
            Parked::Answer(text) => {
                let text = text?;
                app.take_agent_answer(task_id);
                return Some(Answered { text });
            }
            // No overseer turn started: the ladder decides, not a wait with
            // nobody to answer.
            Parked::NoOverseer => {}
        }
    }
    // The next rung is never an open question: one changed approach, then
    // the task fails with a report (`ladder.rs`).
    task.clear_question();
    if ladder::approach_changed(task) {
        ladder::end_ladder(app, task);
        return None;
    }
    task.resume_phase(TaskStatus::Queued, task.updated_at);
    Some(Answered {
        text: "change approach".to_string(),
    })
}

/// What an answer to "attempts keep failing" does besides extending the
/// budget: `change approach` changes how the next attempt works.
pub(in crate::engine) fn settle_exhausted_answer(
    app: &App,
    task: &mut Task,
    idx: usize,
    answer: &str,
) {
    if is_option(answer, "change approach") {
        let settings = app.settings.read().unwrap().clone();
        let attempt_n = task.attempts[idx].n;
        ladder::change_approach(task, &settings, attempt_n);
        task.reset_rate_pauses();
        // The loop reloads the task from disk before its next attempt.
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
}

/// An answer to a question that parked a loop.
pub(in crate::engine) struct Answered {
    pub text: String,
}

impl App {
    /// An overseer turn ended on "attempts keep failing" and did not answer
    /// it: the owner is not asked. The failure ladder takes its next rung, as
    /// when no overseer turn starts: one changed approach (the parked loop
    /// gets `change approach` as the answer; with no loop it is applied here
    /// and the task queued), then the end (failed, with its report and one
    /// notice) with the parked loop cancelled.
    /// Returns whether it queued the task for a new loop.
    pub(in crate::engine) fn step_unanswered_exhausted(&self, task: &mut Task) -> bool {
        if !ladder::approach_changed(task) {
            let attempt_n = task
                .attempts
                .iter()
                .rfind(|a| a.stage == Stage::Implement)
                .map(|a| a.n);
            let live = self.controls.lock().unwrap().get(&task.id).map(|c| {
                c.signals
                    .pending_answer
                    .lock()
                    .unwrap()
                    .take()
                    .is_some_and(|tx| {
                        tx.send(Reply {
                            text: "change approach".to_string(),
                            default_evidence: None,
                            like: None,
                        })
                        .is_ok()
                    })
            });
            match (live, attempt_n) {
                // The parked loop settles the answer and saves.
                (Some(true), _) => return false,
                // A loop between two waits: its own copy decides next.
                (Some(false), _) => {
                    task.release_question_hold(false);
                    self.save_task_logged(task);
                    self.broadcast_task(task);
                    return false;
                }
                (None, Some(n)) => {
                    let settings = self.settings.read().unwrap().clone();
                    ladder::change_approach(task, &settings, n);
                    task.clear_question();
                    task.resume_phase(TaskStatus::Queued, now_ms());
                    self.save_task_logged(task);
                    self.broadcast_task(task);
                    return true;
                }
                (None, None) => {}
            }
        }
        ladder::end_ladder(self, task);
        self.notify_task_failed(task);
        if let Some(control) = self.controls.lock().unwrap().get(&task.id) {
            control.signals.cancel.cancel();
        }
        false
    }
}
