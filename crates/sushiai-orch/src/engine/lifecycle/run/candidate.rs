use super::end::CycleEnd;
use crate::engine::lifecycle::carry::attempt;
use crate::engine::lifecycle::evidence;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::inspection::Deliverable;
use crate::engine::lifecycle::review::ReviewRound;
use crate::engine::lifecycle::review_admission;
use crate::engine::lifecycle::verification::{self, Verification};
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::task::Task;
use std::sync::Arc;

pub(super) struct ReviewCandidate {
    run: RunReceipt,
    delivered: Deliverable,
}

impl ReviewCandidate {
    pub(super) fn new(run: RunReceipt, delivered: Deliverable) -> Self {
        Self { run, delivered }
    }

    pub(super) async fn prepare(
        mut self,
        app: &Arc<App>,
        task: &mut Task,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Result<ReviewRound, CycleEnd> {
        attempt::carry(
            app,
            task,
            &self.run,
            &mut self.delivered.changed,
            &mut QuestionSlot::new(signals, permit),
        )
        .await?;
        let stale = evidence::capture(app, task, &self.run).await;
        let verified = match verification::run(app, task, &self.run, signals, permit).await {
            Verification::Ready(results) => results,
            Verification::Failed(failure) => return Err(CycleEnd::Failed(failure)),
            Verification::Retry(extra) => return Err(CycleEnd::Retry(extra)),
            Verification::Changed => return Err(CycleEnd::Retry(0)),
            Verification::Stopped => return Err(CycleEnd::Stopped),
        };
        evidence::validation::validate(app, task, &self.run, stale, &signals.cancel).await?;
        review_admission::prepare(app, task, &self.run, signals, permit).await?;
        Ok(ReviewRound::new(self.run, self.delivered.report, verified))
    }
}
