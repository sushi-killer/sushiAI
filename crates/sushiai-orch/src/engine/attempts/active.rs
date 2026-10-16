use crate::engine::attempts::pricing::guard_line;
use crate::engine::attempts::run::{RunIdentity, RunInvocation};
use crate::engine::attempts::stream::RunStream;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::harness::native::io::{NativeRun, ProcessEvent};
use crate::harness::native::redaction::redact;
use crate::store::log::append_line;
use std::collections::HashMap;
use std::sync::Arc;

/// The live process and observations retained until completion or cancellation.
pub(super) struct ActiveRun {
    pub(super) process: NativeRun,
    pub(super) stream: RunStream,
    pub(super) guard: Guard,
    pub(super) redaction: HashMap<String, String>,
    pub(super) started_at: i64,
}

impl ActiveRun {
    pub(super) async fn drive(
        &mut self,
        app: &Arc<App>,
        identity: &RunIdentity<'_>,
        invocation: &RunInvocation<'_>,
        cancel: &CancelToken,
    ) -> Result<(), RunError> {
        let req = invocation.request;
        loop {
            match self.process.next(cancel, &mut self.guard).await {
                ProcessEvent::Cancelled => {
                    return Err(self.cancel(app, identity, invocation).await)
                }
                ProcessEvent::Tripped(trip) => {
                    self.process.stop().await;
                    trip.apply(&mut self.stream.outcome);
                    break;
                }
                ProcessEvent::Stdout(line) => {
                    let line = redact(&line, &self.redaction);
                    append_line(invocation.events, &line);
                    if let Some(note) =
                        harness::feed_stream_line(req.harness, &line, &mut self.stream.outcome)
                    {
                        app.broadcast_log(identity.id, identity.attempt_n, note);
                    }
                    if let Some(trip) = guard_line(
                        app,
                        &mut self.guard,
                        (req.harness, req.model),
                        &line,
                        &self.stream.outcome,
                    ) {
                        self.process.stop().await;
                        trip.apply(&mut self.stream.outcome);
                        break;
                    }
                    if identity.track != RunTrack::No {
                        self.stream
                            .observe_attempt(app, identity.id, identity.attempt_n, &line)
                            .await;
                    }
                }
                ProcessEvent::Stderr(line) => {
                    self.guard.heard();
                    let line = redact(&line, &self.redaction);
                    append_line(invocation.events, &format!("[stderr] {line}"));
                    self.stream.stderr(&line);
                }
                ProcessEvent::Exit(status) => {
                    self.stream
                        .exit(req.harness, identity.track != RunTrack::No, status)?;
                    break;
                }
            }
        }
        Ok(())
    }
}
