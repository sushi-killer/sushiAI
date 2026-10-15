use super::admission::back_to_review;
use super::final_results::{final_failure_detail, FinalFailures};
use super::retry::after_failure;
use super::Tail;
use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::decision::record_failure;
use crate::engine::graph::base::apply_base_check_answer;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::lifecycle::base_wait::{carry_after_base_moves, BaseRetry};
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::rate_limit::counted_attempts;
use crate::engine::state::App;
use crate::engine::verification::base::answers::{
    base_retry_requested, is_kept, record_owner_base_check,
};
use crate::engine::verification::base::questions::pre_existing_question;
use crate::policy::settings::Settings;
use crate::task::{FailureKind, Question, Task, TaskStatus, PRE_EXISTING_RETRY};
use std::sync::Arc;

pub(super) struct FinalQuestion<'a> {
    files: &'a AttemptFiles,
    question: Question,
    failures: FinalFailures,
}

impl<'a> FinalQuestion<'a> {
    pub(super) fn new(files: &'a AttemptFiles, task: &Task, failures: FinalFailures) -> Self {
        let question = pre_existing_question(&failures.pre_existing, &task.base_sha);
        Self {
            files,
            question,
            failures,
        }
    }

    pub(super) async fn settle(
        self,
        app: &Arc<App>,
        task: &mut Task,
        budget: &mut u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Option<Tail> {
        let answer = match self.answer(app, task, slot).await {
            Ok(answer) => answer,
            Err(WaitEnd::Changed) => {
                slot.release();
                return Some(Tail::Continue);
            }
            Err(WaitEnd::Stopped) => {
                slot.release();
                app.finish_task_loop(&task.id);
                return Some(Tail::Return);
            }
        };
        let settings = self.apply_answer(app, task, &answer);
        let kept_all = !self.question.commands.is_empty()
            && self
                .question
                .commands
                .iter()
                .all(|command| is_kept(task, command));
        if kept_all {
            let mut details: Vec<String> = self
                .failures
                .results
                .iter()
                .filter(|result| self.question.commands.contains(&result.command))
                .map(final_failure_detail)
                .collect();
            let ordinary = self.failures.ordinary_detail();
            if !ordinary.is_empty() {
                details.push(ordinary);
            }
            return Some(
                after_failure(
                    app,
                    task,
                    crate::engine::decision::failure(FailureKind::Verify, details.join("\n\n")),
                    budget,
                    slot,
                )
                .await,
            );
        }
        if self
            .question
            .commands
            .iter()
            .any(|command| task.final_verify.contains(command))
        {
            return Some(self.retry_base(app, task, budget, slot).await);
        }
        // Every listed check is gone: its discarded review must be repeated.
        crate::task::transitions::reopen(task, self.files.idx, now_ms());
        task.resume_phase(TaskStatus::Running, task.updated_at);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        if self.failures.ordinary.is_empty()
            && task.attempts[self.files.idx].review.is_none()
            && !settings.review.is_empty()
        {
            task.record_decision(format!("Orchestrator: attempt {} reviewed again because its review was discarded by a check that failed on the base", self.files.n));
            return Some(back_to_review(app, task, self.files.idx, slot));
        }
        if !self.failures.ordinary.is_empty() {
            return Some(
                after_failure(
                    app,
                    task,
                    crate::engine::decision::failure(
                        FailureKind::Verify,
                        self.failures.ordinary_detail(),
                    ),
                    budget,
                    slot,
                )
                .await,
            );
        }
        None
    }

    async fn answer(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        slot: &mut QuestionSlot<'_>,
    ) -> Result<String, WaitEnd> {
        let mut detail: Vec<String> = self
            .failures
            .pre_existing
            .iter()
            .map(|base| {
                self.failures
                    .results
                    .iter()
                    .find(|failed| failed.command == base.command)
                    .map(final_failure_detail)
                    .unwrap_or_default()
            })
            .collect();
        let ordinary = self.failures.ordinary_detail();
        if !ordinary.is_empty() {
            detail.push(ordinary);
        }
        record_failure(
            task,
            self.files.idx,
            FailureKind::Verify,
            detail.join("\n\n"),
        );
        let assumed = task.assumptions.len();
        let already_retried = base_retry_requested(task, &self.question.commands, &task.base_sha);
        let answer = if already_retried {
            PRE_EXISTING_RETRY.to_string()
        } else {
            task.ask(self.question.clone(), task.updated_at);
            task.resume_phase(TaskStatus::Waiting, task.updated_at);
            task.touch(now_ms());
            slot.answer(app, task).await?
        };
        if !already_retried {
            let base_sha = task.base_sha.clone();
            let policy_answered = task.assumptions.len() > assumed;
            record_owner_base_check(
                task,
                &self.question.commands,
                &base_sha,
                &answer,
                policy_answered,
            );
        }
        Ok(answer)
    }

    fn apply_answer(&self, app: &App, task: &mut Task, answer: &str) -> Settings {
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        // A refused removal stays a check and counts as a retry.
        if apply_base_check_answer(&settings, task, &self.question, answer, origin) {
            app.verify_cache.lock().unwrap().remove(&task.id);
        }
        settings
    }

    async fn retry_base(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        budget: &mut u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        match carry_after_base_moves(app, task, &self.files.worktree, slot).await {
            BaseRetry::Ready => {}
            BaseRetry::Conflict(detail) => {
                record_failure(task, self.files.idx, FailureKind::Conflict, detail)
            }
            BaseRetry::Changed => {
                slot.release();
                return Tail::Continue;
            }
            BaseRetry::Stopped => return self.files.interrupt(app, task, slot),
        }
        *budget = (*budget).max(counted_attempts(task) + 1);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        slot.release();
        Tail::Continue
    }
}

#[cfg(test)]
#[path = "tests/base_question.rs"]
mod tests;
