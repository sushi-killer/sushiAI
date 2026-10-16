mod fast;
mod gates;

use crate::engine::decision::failure;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::plan_gate;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::task::checks::results::refresh_criteria_results;
use crate::task::{Failure, FailureKind, Task, VerifyOutcome};
use std::sync::Arc;

pub(super) enum Verification {
    Ready(Vec<VerifyOutcome>),
    Failed(Failure),
    Retry(u32),
    Changed,
    Stopped,
}

pub(super) async fn run(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Verification {
    let cancel = &signals.cancel;
    if let Err(error) = apply_pending_amendment(app, task, &signals.pending_amend, cancel).await {
        eprintln!("orch: applying amendment to {} failed: {error}", task.id);
        return Verification::Stopped;
    }
    if let Some(detail) = plan_gate::checklist::run(
        app,
        task,
        &run.worktree,
        &run.run_dir.join("checklist"),
        cancel,
    )
    .await
    {
        return Verification::Failed(failure(FailureKind::Verify, detail));
    }
    if cancel.is_cancelled() {
        interrupt_attempt(app, task, run.idx);
        return Verification::Stopped;
    }
    let mut results =
        match fast::verify(app, task, run, &mut QuestionSlot::new(signals, permit)).await {
            Ok(results) => results,
            Err(end) => return end,
        };
    match gates::verify(app, task, run, cancel).await {
        Ok(gated) => results.extend(gated),
        Err(end) => return end,
    }
    refresh_criteria_results(task, run.idx);
    Verification::Ready(results)
}
