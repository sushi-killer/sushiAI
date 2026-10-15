mod denials;
mod files;
mod outcome;

use crate::engine::decision::failure;
use crate::engine::lifecycle::changes::capture_run_interruption;
use crate::engine::lifecycle::native_result::ObservedRun;
use crate::engine::lifecycle::report::{self, ImplementerReport};
use crate::engine::lifecycle::reported_question::{self, Action};
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::engine::test_claim::run::ClaimReview;
use crate::task::{Failure, Task};
use std::sync::Arc;

pub(super) struct Deliverable {
    pub(super) report: ImplementerReport,
    pub(super) changed: Vec<String>,
}

pub(super) enum InspectionEnd {
    Failed(Failure),
    Retry(u32),
    Stopped,
}

impl From<WaitEnd> for InspectionEnd {
    fn from(value: WaitEnd) -> Self {
        match value {
            WaitEnd::Changed => Self::Retry(0),
            WaitEnd::Stopped => Self::Stopped,
        }
    }
}

pub(super) async fn inspect(
    app: &Arc<App>,
    task: &mut Task,
    observed: &ObservedRun,
    budget: u32,
    slot: &mut QuestionSlot<'_>,
) -> Result<Deliverable, InspectionEnd> {
    if let Some((kind, detail)) =
        capture_run_interruption(task, &observed.run, &observed.outcome).await
    {
        return Err(InspectionEnd::Failed(failure(kind, detail)));
    }
    denials::settle(app, task, observed, slot).await?;
    let report = report::record(task, observed.run.idx, &observed.outcome);
    let changed = files::protect(task, observed).await;
    if let Some(report) = report
        .report
        .as_ref()
        .filter(|report| !report.test_claims.is_empty())
    {
        let cancel = slot.cancel().clone();
        ClaimReview::new(task, observed.run.idx, &cancel)
            .run(app, task, &report.test_claims, slot)
            .await?;
    }
    match reported_question::resolve(app, task, &report, budget, slot).await {
        Action::Proceed => {}
        Action::Retry(extra) => return Err(InspectionEnd::Retry(extra)),
        Action::Stop => return Err(InspectionEnd::Stopped),
    }
    outcome::settle(app, task, observed, &changed, slot).await?;
    Ok(Deliverable { report, changed })
}
