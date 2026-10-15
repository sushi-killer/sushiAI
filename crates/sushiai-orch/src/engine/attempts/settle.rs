use crate::engine::attempts::ledger::record_run;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::pricing::finalize_cost;
use crate::engine::attempts::pricing::price_resumed_attempt;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::persistence::persist_attempt_observation;
use crate::engine::state::App;
use crate::harness;
use crate::harness::native::error::RunError;
use crate::harness::native::io::NativeRun;
use std::path::Path;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub(super) fn complete_run(
    app: &App,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    req: &harness::RunRequest<'_>,
    tag: &CostTag,
    process: &NativeRun,
    mut outcome: harness::RunOutcome,
    started_at: i64,
) -> harness::RunOutcome {
    finalize_cost(app, req.harness, req.model, &mut outcome);
    if track != RunTrack::No {
        price_resumed_attempt(
            app,
            task_id,
            attempt_n,
            req.harness,
            req.resume,
            &mut outcome,
        );
    }
    outcome.fingerprint = Some(process.fingerprint(&outcome, req));
    record_run(
        app,
        task_id,
        attempt_n,
        tag,
        (req.harness, req.model, req.resume.is_some()),
        &outcome,
        started_at,
    );
    outcome
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn cancel_run(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    req: &harness::RunRequest<'_>,
    tag: &CostTag,
    events_path: &Path,
    process: &mut NativeRun,
    outcome: &harness::RunOutcome,
    started_at: i64,
) -> RunError {
    if track == RunTrack::Resumable && app.runs_survive_shutdown() {
        return RunError::Cancelled;
    }
    process.stop().await;
    let fp = process.fingerprint(outcome, req);
    let mut partial = outcome.clone();
    finalize_cost(app, req.harness, req.model, &mut partial);
    if track != RunTrack::No {
        price_resumed_attempt(
            app,
            task_id,
            attempt_n,
            req.harness,
            req.resume,
            &mut partial,
        );
    }
    partial.fingerprint = Some(fp.clone());
    record_run(
        app,
        task_id,
        attempt_n,
        tag,
        (req.harness, req.model, req.resume.is_some()),
        &partial,
        started_at,
    );
    if track != RunTrack::No {
        persist_attempt_observation(
            app,
            task_id,
            attempt_n,
            crate::task::attempts::RunObservation::Fingerprint(fp),
        )
        .await;
    } else {
        crate::harness::run_files::write_cancelled_fingerprint(events_path, &fp);
    }
    RunError::Cancelled
}
