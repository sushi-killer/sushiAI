use crate::engine::attempts::active::ActiveRun;
use crate::engine::attempts::ledger::record_run;
use crate::engine::attempts::pricing::{finalize_cost, price_resumed_attempt, ProviderRun};
use crate::engine::attempts::run::{RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness::native::error::RunError;
use crate::harness::{RunOutcome, RunRequest};
use crate::task::attempt::Fingerprint;
use std::sync::Arc;

impl ActiveRun {
    pub(super) fn complete(
        mut self,
        app: &App,
        identity: &RunIdentity<'_>,
        req: &RunRequest<'_>,
    ) -> RunOutcome {
        self.settle(app, identity, req);
        self.stream.outcome
    }

    fn settle(
        &mut self,
        app: &App,
        identity: &RunIdentity<'_>,
        req: &RunRequest<'_>,
    ) -> Fingerprint {
        let outcome = &mut self.stream.outcome;
        finalize_cost(app, req.harness, req.model, outcome);
        if identity.track != RunTrack::No {
            price_resumed_attempt(
                app,
                identity.id,
                identity.attempt_n,
                ProviderRun::from(req),
                outcome,
            );
        }
        let fingerprint = self.process.fingerprint(outcome, req);
        outcome.fingerprint = Some(fingerprint.clone());
        record_run(
            app,
            identity,
            ProviderRun::from(req),
            outcome,
            self.started_at,
        );
        fingerprint
    }

    pub(super) async fn cancel(
        &mut self,
        app: &Arc<App>,
        identity: &RunIdentity<'_>,
        invocation: &RunInvocation<'_>,
    ) -> RunError {
        if identity.track == RunTrack::Resumable && app.runs_survive_shutdown() {
            return RunError::Cancelled;
        }
        self.process.stop().await;
        let fingerprint = self.settle(app, identity, invocation.request);
        if identity.track != RunTrack::No {
            let prices = app.settings.read().unwrap().prices.clone();
            if let Err(error) = app.update_task(identity.id, |task| {
                if identity.track == RunTrack::Resumable {
                    if let Some(index) = task
                        .attempts
                        .iter()
                        .position(|attempt| attempt.n == identity.attempt_n)
                    {
                        crate::engine::cost::settle_unfinished_cost(
                            task, index, &app.store, &prices,
                        );
                    }
                }
                crate::task::attempts::observe_run(
                    task,
                    identity.attempt_n,
                    crate::task::attempts::RunObservation::Fingerprint(fingerprint),
                    crate::clock::now_ms(),
                );
            }) {
                eprintln!(
                    "orch: saving cancelled run for {} failed: {error}",
                    identity.id
                );
            }
        } else {
            crate::harness::run_files::write_cancelled_fingerprint(invocation.events, &fingerprint);
        }
        RunError::Cancelled
    }
}
