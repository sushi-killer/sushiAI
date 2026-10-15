use super::InspectionEnd;
use crate::clock::now_ms;
use crate::engine::decision::failure;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::lifecycle::native_result::ObservedRun;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::rate_limit::{pause_for_rate_limit, rate_limit_wait_secs};
use crate::engine::state::App;
use crate::task::{FailureKind, Task};
use std::sync::Arc;

pub(super) async fn settle(
    app: &Arc<App>,
    task: &mut Task,
    observed: &ObservedRun,
    changed: &[String],
    slot: &mut QuestionSlot<'_>,
) -> Result<(), InspectionEnd> {
    if changed.is_empty() {
        let provider_failure = observed.outcome.provider_failure(now_ms() / 1000);
        if let Some((error, wait)) = observed
            .outcome
            .error
            .as_deref()
            .zip(provider_failure)
            .and_then(|(error, failure)| {
                rate_limit_wait_secs(failure, task.queue.rate_pauses).map(|wait| (error, wait))
            })
        {
            return if pause_for_rate_limit(app, task, error, wait, slot).await {
                Err(InspectionEnd::Retry(0))
            } else {
                Err(InspectionEnd::Stopped)
            };
        }
    }
    if observed.outcome.error.is_none() {
        task.reset_rate_pauses();
    }
    if changed.is_empty() {
        let (kind, detail) = match &observed.outcome.error {
            Some(error) => (FailureKind::Error, error.clone()),
            None => (FailureKind::NoDeliverable, "No files changed.".to_string()),
        };
        return Err(InspectionEnd::Failed(failure(kind, detail)));
    }
    if slot.cancel().is_cancelled() {
        interrupt_attempt(app, task, observed.run.idx);
        return Err(InspectionEnd::Stopped);
    }
    Ok(())
}
