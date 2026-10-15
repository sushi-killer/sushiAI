use super::candidate::ReviewCandidate;
use super::end::{route_tail, CycleEnd};
use crate::engine::decision::failure;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::lifecycle::completion::finish::ReviewedAttempt;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::execution::{
    self, ExecutedAttempt, ExecutionMode, ExecutionStep, RunReceipt,
};
use crate::engine::lifecycle::final_checks::FinalChecks;
use crate::engine::lifecycle::inspection::{self, Deliverable};
use crate::engine::lifecycle::native_result::{self, NativeEnd, ObservedRun};
use crate::engine::lifecycle::review::{ReviewEnd, ReviewRound};
use crate::engine::lifecycle::round::LoopProgress;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::recovery::resumable_attempt;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::policy::pipelines::task::{compiled, TaskNext, TaskOutcome, TaskStep};
use crate::task::{FailureKind, Task};
use std::sync::Arc;

/// Products crossing the existing attempt operations; no operation can manufacture
/// the input of a later safety gate by changing a YAML destination.
enum Receipt {
    Unexecuted,
    Native(Box<ExecutedAttempt>),
    Observed(Box<ObservedRun>),
    Inspected(Box<RunReceipt>, Box<Deliverable>),
    Review(Box<RunReceipt>, Box<ReviewRound>),
    Reviewed(Box<RunReceipt>, Option<FinalChecks>),
}

pub(super) async fn execute(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
    progress: &mut LoopProgress,
) -> Result<Tail, CycleEnd> {
    let flow = compiled();
    let mut step = flow.entry();
    let mut receipt = Receipt::Unexecuted;
    loop {
        let result = match step {
            TaskStep::Execute => {
                if !matches!(receipt, Receipt::Unexecuted) {
                    unreachable!("execution requires an unexecuted attempt");
                }
                Receipt::execute(app, task, signals, permit, progress).await
            }
            TaskStep::Settle => receipt.settle(app, task, signals).await,
            TaskStep::Inspect => {
                receipt
                    .inspect(
                        app,
                        task,
                        progress.budget,
                        &mut QuestionSlot::new(signals, permit),
                    )
                    .await
            }
            TaskStep::Qualify => receipt.qualify(app, task, signals, permit).await,
            TaskStep::Review => {
                receipt
                    .review(app, task, &mut QuestionSlot::new(signals, permit))
                    .await
            }
            TaskStep::Complete => {
                let tail = receipt
                    .complete(
                        app,
                        task,
                        &mut progress.budget,
                        &mut QuestionSlot::new(signals, permit),
                    )
                    .await;
                return Ok(route_tail(flow, tail));
            }
        };
        receipt = match result {
            Ok(receipt) => receipt,
            Err(end) => return Err(end.route(flow, step)),
        };
        let TaskNext::Step(next) = flow.next(step, TaskOutcome::Ready) else {
            unreachable!("a ready receipt requires its consumer");
        };
        step = next;
    }
}

impl Receipt {
    async fn execute(
        app: &Arc<App>,
        task: &mut Task,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
        progress: &LoopProgress,
    ) -> Result<Self, CycleEnd> {
        let mode = match resumable_attempt(app, task) {
            Some(index) => ExecutionMode::Adopt(index),
            None => ExecutionMode::Start(progress.parent_failure.as_deref()),
        };
        match execution::execute(app, task, signals, permit, mode).await {
            ExecutionStep::Executed(run) => Ok(Self::Native(run)),
            ExecutionStep::Retry => Err(CycleEnd::Retry(0)),
            ExecutionStep::Stop => Err(CycleEnd::Finished),
        }
    }

    async fn settle(
        self,
        app: &Arc<App>,
        task: &mut Task,
        signals: &TaskSignals,
    ) -> Result<Self, CycleEnd> {
        let Self::Native(executed) = self else {
            unreachable!("settlement requires a native receipt");
        };
        let observed = match native_result::settle(app, task, *executed) {
            Ok(observed) => Box::new(observed),
            Err(NativeEnd::Changed(observed)) => {
                observed.after_amendments(app, task, signals).await?
            }
            Err(end) => return Err(end.into()),
        };
        Ok(Self::Observed(observed))
    }

    async fn inspect(
        self,
        app: &Arc<App>,
        task: &mut Task,
        budget: u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Result<Self, CycleEnd> {
        let Self::Observed(observed) = self else {
            unreachable!("inspection requires settled native facts");
        };
        let delivered = inspection::inspect(app, task, &observed, budget, slot).await?;
        Ok(Self::Inspected(Box::new(observed.run), Box::new(delivered)))
    }

    async fn qualify(
        self,
        app: &Arc<App>,
        task: &mut Task,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Result<Self, CycleEnd> {
        let Self::Inspected(run, delivered) = self else {
            unreachable!("qualification requires an inspected deliverable");
        };
        let review = ReviewCandidate::new((*run).clone(), *delivered)
            .prepare(app, task, signals, permit)
            .await?;
        Ok(Self::Review(run, Box::new(review)))
    }

    async fn review(
        self,
        app: &Arc<App>,
        task: &mut Task,
        slot: &mut QuestionSlot<'_>,
    ) -> Result<Self, CycleEnd> {
        let Self::Review(run, review) = self else {
            unreachable!("review requires verified evidence and admission");
        };
        let finished = match review.run(app, task, slot).await {
            ReviewEnd::Finished(finished) => finished,
            ReviewEnd::Changed => return Err(CycleEnd::Changed),
            ReviewEnd::Stopped => return Err(CycleEnd::Stopped),
            ReviewEnd::Failed(detail) => {
                return Err(CycleEnd::Failed(failure(FailureKind::Review, detail)));
            }
        };
        Ok(Self::Reviewed(run, finished))
    }

    async fn complete(
        self,
        app: &Arc<App>,
        task: &mut Task,
        budget: &mut u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        let Self::Reviewed(run, finished) = self else {
            unreachable!("completion requires a settled review");
        };
        let files = AttemptFiles::new(run.idx, run.attempt_n, run.worktree, run.run_dir);
        ReviewedAttempt::new(files, &run.settings, finished)
            .finish(app, task, budget, slot)
            .await
    }
}

#[cfg(test)]
mod tests;
