use crate::clock::now_ms;
use crate::engine::answer_policy::publication::PolicyResult;
use crate::engine::answer_policy::try_policy_answer;
use crate::engine::ladder;
use crate::engine::questions::parking::{Parked, QuestionSlot, WaitEnd};
use crate::engine::questions::reply::Reply;
use crate::engine::state::App;
use crate::task::is_option;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;

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
    slot: &mut QuestionSlot<'_>,
) -> Result<Answered, WaitEnd> {
    let id = task.id.clone();
    let task_id = id.as_str();
    if task.queue.park.is_some() || task.status == TaskStatus::Failed {
        task.touch(now_ms());
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        app.advance_queue(&task.repo);
        return Err(WaitEnd::Stopped);
    }
    match try_policy_answer(app, task, slot.cancel()).await {
        PolicyResult::Answered(text) => return Ok(Answered { text }),
        PolicyResult::Stale => return Err(WaitEnd::Changed),
        PolicyResult::Stopped => return Err(WaitEnd::Stopped),
        PolicyResult::Open => {}
    }
    let auto_answer = app.settings.read().unwrap().auto_answer;
    if (task.loop_state.non_owner_continues as usize) < MAX_ORCHESTRATOR_CONTINUES && auto_answer {
        if !slot.prepare(app, task) {
            return Err(WaitEnd::Stopped);
        }
        match slot.park_with(app, task, true).await {
            Parked::Answer(text) => {
                let text = text.ok_or(WaitEnd::Stopped)?;
                app.take_agent_answer(task_id);
                return Ok(Answered { text });
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
        return Err(WaitEnd::Stopped);
    }
    task.resume_phase(TaskStatus::Queued, task.updated_at);
    Ok(Answered {
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
                    self.write_exhausted_step(task, |fresh| {
                        fresh.release_question_hold(false);
                    });
                    return false;
                }
                (None, Some(n)) => {
                    let settings = self.settings.read().unwrap().clone();
                    return self.write_exhausted_step(task, |fresh| {
                        ladder::change_approach(fresh, &settings, n);
                        fresh.clear_question();
                        fresh.resume_phase(TaskStatus::Queued, now_ms());
                    });
                }
                (None, None) => {}
            }
        }
        if self.write_exhausted_step(task, ladder::end_ladder_on) {
            self.notify_task_failed(task);
            if let Some(control) = self.controls.lock().unwrap().get(&task.id) {
                control.signals.cancel.cancel();
            }
        }
        false
    }

    /// Applies one rung to the task as stored now, under its lock, while it still holds the
    /// question `task` was read with; `task` becomes the saved copy. `false` when another
    /// writer settled the question first: nothing is written.
    fn write_exhausted_step(&self, task: &mut Task, step: impl FnOnce(&mut Task)) -> bool {
        let key = |t: &Task| {
            t.question
                .as_ref()
                .map(|q| (q.kind, q.text.clone(), q.asked_at))
        };
        let asked = key(task);
        let saved = self.update_task_if(&task.id, |fresh| {
            let same = asked.is_some() && key(fresh) == asked;
            if same {
                step(fresh);
            }
            same
        });
        match saved {
            Some(saved) => {
                *task = saved;
                true
            }
            None => false,
        }
    }
}
