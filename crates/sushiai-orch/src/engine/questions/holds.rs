use crate::clock::now_ms;
use crate::engine::answer_policy::owner_only;
use crate::engine::state::App;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::atomic::Ordering;

/// An "attempts keep failing" question belongs to the failure ladder: it is
/// never handed to the owner (`ladder.rs`).
pub(super) fn ladder_owns(question: &Question) -> bool {
    question.kind == QuestionKind::AttemptsFailing
}

impl App {
    /// Holds the waiting questions an overseer turn of `reason` that has just
    /// started is about to look at: the one named by `task_id` for a
    /// `waiting` or `reminder` wake, every non-owner-only one in `repo` for a
    /// `restart` wake. Owner-only kinds are never held. Returns what it held
    /// as (task id, hold time), so only that hold is released at the turn's
    /// end. Saved and broadcast.
    pub(crate) fn hold_for_overseer(
        &self,
        repo: &str,
        reason: &str,
        task_id: Option<&str>,
    ) -> Vec<(String, i64)> {
        let tasks: Vec<Task> = match (reason, task_id) {
            ("waiting" | "reminder", Some(id)) => self
                .store
                .load_task(id)
                .ok()
                .flatten()
                .into_iter()
                .collect(),
            ("restart", _) => self
                .store
                .list_tasks()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.repo == repo)
                .collect(),
            _ => return Vec::new(),
        };
        let at = now_ms();
        let mut held = Vec::new();
        for mut task in tasks {
            if task.status != TaskStatus::Waiting || task.archived {
                continue;
            }
            let Some(_) = task.question.as_ref().filter(|q| !owner_only(q.kind)) else {
                continue;
            };
            task.hold_question(at);
            held.push((task.id.clone(), at));
            self.save_task_logged(&task);
            self.broadcast_task(&task);
        }
        held
    }
}

impl App {
    /// Ends the holds a finished turn made, each only if it is still the same
    /// hold (a newer turn or question replaced it otherwise) and no queued
    /// wake for that task is about to hold it again. With `hand_over` the
    /// question the turn left open is the owner's now, also when its hold
    /// ran out (TTL) before the turn ended. An "attempts keep failing"
    /// question takes the ladder's next rung instead.
    pub(crate) fn release_holds(&self, repo: &str, held: &[(String, i64)], hand_over: bool) {
        // A daemon that is stopping hands nothing on: its restart wake does.
        let hand_over = hand_over && !self.shutting_down.load(Ordering::SeqCst);
        for (id, at) in held {
            if crate::engine::chat::wake::wake_queued_for(self, repo, id) {
                continue;
            }
            let Ok(Some(mut task)) = self.store.load_task(id) else {
                continue;
            };
            let Some(q) = task.question.as_ref() else {
                continue;
            };
            let ours = q.held_at == Some(*at);
            // Unheld and asked no later than the hold: the same question,
            // whose hold the TTL ended while the turn ran.
            let lapsed = !q.held_for_overseer && q.asked_at.is_none_or(|asked| asked <= *at);
            let asked_at = q.asked_at;
            let waiting = task.status == TaskStatus::Waiting;
            if ours {
                if ladder_owns(q) {
                    if self.step_unanswered_exhausted(&mut task) {
                        self.advance_queue(&task.repo);
                    }
                    continue;
                }
                task.release_question_hold(hand_over && waiting);
                self.save_task_logged(&task);
                self.broadcast_task(&task);
            } else if hand_over && lapsed {
                self.hand_to_owner(id, asked_at);
            }
        }
    }
}

impl App {
    /// Marks task `id`'s waiting question as the owner's (`for_owner`), when
    /// it is still the question asked at `asked_at` and not one the failure
    /// ladder settles. Saved and broadcast only when that changes anything;
    /// whether it did.
    pub(crate) fn hand_to_owner(&self, id: &str, asked_at: Option<i64>) -> bool {
        let lock = self.task_lock(id);
        let task = {
            let _guard = lock.lock().unwrap();
            let Ok(Some(mut task)) = self.store.load_task(id) else {
                return false;
            };
            if !task.hand_question_to_owner(asked_at, now_ms()) {
                return false;
            }
            if let Err(e) = self.store.save_task(&task) {
                eprintln!("orch: saving task {id} failed: {e}");
                return false;
            }
            task
        };
        self.broadcast_task(&task);
        true
    }
}

impl App {
    /// Releases every hold that `keep` does not keep, handing the waiting
    /// question to the owner in the same save when `hand_over` says so.
    fn release_holds_where(
        &self,
        keep: impl Fn(&Question) -> bool,
        hand_over: impl Fn(&Task) -> bool,
    ) {
        for mut task in self.store.list_tasks().unwrap_or_default() {
            let owners = task.status == TaskStatus::Waiting && hand_over(&task);
            let Some(q) = task
                .question
                .as_ref()
                .filter(|q| q.held_for_overseer && !keep(q))
            else {
                continue;
            };
            // An "attempts keep failing" question is never left open: with
            // the hold gone and no queued wake to answer it, the ladder takes
            // its next rung. A queued wake keeps the question for its turn.
            if ladder_owns(q)
                && !crate::engine::chat::wake::wake_queued_for(self, &task.repo, &task.id)
            {
                if self.step_unanswered_exhausted(&mut task) {
                    self.advance_queue(&task.repo);
                }
                continue;
            }
            task.release_question_hold(owners);
            self.save_task_logged(&task);
            self.broadcast_task(&task);
        }
    }
}

impl App {
    /// A daemon that stopped mid-turn left its holds behind: no turn runs now.
    /// An "attempts keep failing" question stays open: the restart wake asks
    /// the overseer again, and `retire_ladder_questions` steps the ladder when
    /// nobody will.
    pub(crate) fn release_stale_holds(&self) {
        for mut task in self.store.list_tasks().unwrap_or_default() {
            if task.question.as_ref().is_some_and(|q| q.held_for_overseer) {
                task.release_question_hold(false);
                self.save_task_logged(&task);
                self.broadcast_task(&task);
            }
        }
    }
}

impl App {
    /// Holds older than [`HOLD_TTL_MS`] end, whatever the turn is doing. A
    /// turn still running in the repo hands its questions on when it ends; a
    /// hold that no turn or queued wake owns any more goes to the owner now.
    pub(crate) fn release_expired_holds(&self, now: i64) {
        let busy: std::collections::HashSet<String> =
            self.chat_turns.lock().unwrap().keys().cloned().collect();
        self.release_holds_where(
            |q| q.held_now(now),
            |task| {
                !busy.contains(&task.repo)
                    && !crate::engine::chat::wake::wake_queued_for(self, &task.repo, &task.id)
            },
        );
    }
}

impl App {
    /// A wake of `reason` that cannot run (no route, no repo directory):
    /// nobody takes the questions it was for, so they are the owner's, and a
    /// hold its queued copy left ends. A daemon that is stopping hands nothing
    /// on; its restart wakes the overseer again. An "attempts keep failing"
    /// question is left to the ladder.
    pub(crate) fn hand_over_unwoken(&self, repo: &str, reason: &str, task_id: Option<&str>) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        for (id, _) in self.wake_questions(repo, reason, task_id) {
            let Ok(Some(mut task)) = self.store.load_task(&id) else {
                continue;
            };
            let Some(question) = task.question.as_ref() else {
                continue;
            };
            if ladder_owns(question) || (!question.held_for_overseer && question.for_owner) {
                continue;
            }
            task.hand_over_question(now_ms());
            self.save_task_logged(&task);
            self.broadcast_task(&task);
        }
    }
}
