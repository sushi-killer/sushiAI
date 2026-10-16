mod finish;
mod rounds;
mod routing;
#[cfg(test)]
mod tests;
mod verdict;

use crate::engine::ladder;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::final_checks::{BackgroundFinal, FinalChecks};
use crate::engine::lifecycle::report::ImplementerReport;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::state::App;
use crate::task::attempt::VerifyOutcome;
use crate::task::review::ReviewResult;
use crate::task::state::Task;
use rounds::ReviewCycle;
use std::sync::Arc;

pub(in crate::engine) struct ReviewRound {
    run: RunReceipt,
    report: ImplementerReport,
    verified: Vec<VerifyOutcome>,
    verdict: Option<ReviewResult>,
    finals: Option<BackgroundFinal>,
}

pub(in crate::engine) enum ReviewEnd {
    Finished(Option<FinalChecks>),
    Failed(String),
    Changed,
    Stopped,
}

pub(super) enum Turn {
    Continue,
    Finished,
}

impl ReviewRound {
    pub(in crate::engine) fn new(
        run: RunReceipt,
        report: ImplementerReport,
        verified: Vec<VerifyOutcome>,
    ) -> Self {
        Self {
            run,
            report,
            verified,
            verdict: None,
            finals: None,
        }
    }

    pub(in crate::engine) async fn run(
        mut self,
        app: &Arc<App>,
        task: &mut Task,
        slot: &mut QuestionSlot<'_>,
    ) -> ReviewEnd {
        task.record_unreviewed(false);
        ladder::inherit_unreviewed(app, task);
        let cancel = slot.cancel().clone();
        let base = task.base_sha.clone();
        match self.select_route(app, task, slot).await {
            Ok(Some(route)) => {
                let finals = BackgroundFinal::start(
                    app,
                    task,
                    &self.run.run_dir,
                    &self.run.settings,
                    &cancel,
                );
                let mut cycle =
                    ReviewCycle::new(route, base, cancel.clone(), finals.review_cancel.clone());
                self.finals = Some(finals);
                if let Err(end) = self.turns(app, task, &mut cycle).await {
                    return end;
                }
            }
            Ok(None) => {}
            Err(end) => return end,
        }
        self.finish(app, task, &cancel).await
    }
}
