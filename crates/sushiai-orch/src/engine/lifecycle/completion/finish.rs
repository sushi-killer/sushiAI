use super::base_question::FinalQuestion;
use super::final_results::{FinalCheckRun, FinalFailures};
use super::retry::after_failure;
use super::Tail;
use crate::engine::delivery::pr::repair::step::{settle_step, StepOutcome};
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::lifecycle::final_checks::{run_final_checks, FinalChecks};
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::engine::verification::context::{
    record_check_decisions, record_check_outcomes, CheckContext,
};
use crate::engine::verification::healing::preflight::settle_base_preflight;
use crate::engine::verification::runner::CheckRunner;
use crate::engine::verification::scoping::{deferred_line, split_deferred};
use crate::policy::settings::Settings;
use crate::task::attempts::{self as observations, VerifyObservation};
use crate::task::{FailureKind, Task};
use std::sync::Arc;

/// Accepted attempt files, their configuration snapshot and any concurrent finals.
pub(in crate::engine) struct ReviewedAttempt<'a> {
    files: AttemptFiles,
    settings: &'a Settings,
    finished: Option<FinalChecks>,
}

impl<'a> ReviewedAttempt<'a> {
    pub(in crate::engine) fn new(
        files: AttemptFiles,
        settings: &'a Settings,
        finished: Option<FinalChecks>,
    ) -> Self {
        Self {
            files,
            settings,
            finished,
        }
    }

    pub(in crate::engine) async fn finish(
        self,
        app: &Arc<App>,
        task: &mut Task,
        budget: &mut u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        let cancel = slot.cancel().clone();
        match settle_base_preflight(app, task, &cancel).await {
            Ok(()) => {}
            Err(WaitEnd::Changed) => {
                slot.release();
                return Tail::Continue;
            }
            Err(WaitEnd::Stopped) => return self.files.interrupt(app, task, slot),
        }
        let (commands, deferred) = split_deferred(app, self.settings, task);
        record_check_decisions(
            task,
            deferred
                .iter()
                .map(|command| deferred_line(command))
                .collect(),
        );
        if !commands.is_empty() {
            let final_checks = match self.finished {
                Some(finished) => finished,
                None => self.files.run_finals(app, task, &commands, &cancel).await,
            };
            record_check_decisions(task, final_checks.skipped);
            record_check_outcomes(app, task, &final_checks.results);
            if cancel.is_cancelled() {
                return self.files.interrupt(app, task, slot);
            }
            observations::record_verify(
                task,
                self.files.idx,
                VerifyObservation::Appended(final_checks.results.clone()),
            );
            let Some(mut failures) = (FinalCheckRun {
                commands: &commands,
                results: final_checks.results,
            })
            .classify(app, task, &self.files, &cancel)
            .await
            else {
                return self.files.interrupt(app, task, slot);
            };
            if let Some(tail) = self
                .files
                .repair_failures(app, task, &mut failures, slot)
                .await
            {
                return tail;
            }
            if !failures.pre_existing.is_empty() {
                if let Some(tail) = FinalQuestion::new(&self.files, task, failures)
                    .settle(app, task, budget, slot)
                    .await
                {
                    return tail;
                }
            } else if !failures.ordinary.is_empty() {
                return after_failure(
                    app,
                    task,
                    crate::engine::decision::failure(
                        FailureKind::Verify,
                        failures.ordinary_detail(),
                    ),
                    budget,
                    slot,
                )
                .await;
            }
        }
        self.files.finish(app, task, budget, slot).await
    }
}

impl AttemptFiles {
    async fn repair_failures(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        failures: &mut FinalFailures,
        slot: &mut QuestionSlot<'_>,
    ) -> Option<Tail> {
        let unresolved: Vec<_> = failures
            .results
            .iter()
            .filter(|result| {
                failures
                    .ordinary
                    .iter()
                    .chain(&failures.pre_existing)
                    .any(|failed| failed.command == result.command)
            })
            .cloned()
            .collect();
        for failed in unresolved {
            match settle_step(app, task, self, &failed, slot).await {
                StepOutcome::NotRepair => return None,
                StepOutcome::Waived => {
                    failures
                        .ordinary
                        .retain(|result| result.command != failed.command);
                    failures
                        .pre_existing
                        .retain(|result| result.command != failed.command);
                }
                StepOutcome::Retry | StepOutcome::Changed => {
                    slot.release();
                    return Some(Tail::Continue);
                }
                StepOutcome::Waiting | StepOutcome::Stopped => {
                    slot.release();
                    app.finish_task_loop(&task.id);
                    return Some(Tail::Return);
                }
            }
        }
        None
    }
    async fn run_finals(
        &self,
        app: &Arc<App>,
        task: &Task,
        commands: &[String],
        cancel: &crate::harness::cancel::CancelToken,
    ) -> FinalChecks {
        // The verify cache contains the fast check set rather than these finals.
        let sandbox = app.task_sandbox(&task.repo, &self.worktree, &self.run_dir);
        run_final_checks(
            CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
                &self.worktree,
                sandbox.as_ref(),
                &task.base_sha,
                cancel,
            ),
            task,
            commands,
            None,
        )
        .await
    }

    pub(super) fn interrupt(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        interrupt_attempt(app, task, self.idx);
        slot.release();
        app.finish_task_loop(&task.id);
        Tail::Return
    }
}
