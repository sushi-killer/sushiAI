use crate::clock::now_ms;
use crate::engine::attempts::active::ActiveRun;
use crate::engine::attempts::configure::configure_command;
use crate::engine::attempts::ledger::{record_run, CostTag};
use crate::engine::attempts::pricing::ProviderRun;
use crate::engine::attempts::secrets::{run_secrets, wait_for_app_secrets};
use crate::engine::attempts::stream::RunStream;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::persistence::persist_attempt_observation;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::harness::native::io::SpawnedRun;
use crate::harness::native::launch::NativeCommand;
use crate::harness::run_files::RunFiles;
use crate::harness::{OutputSchema, RunOutcome, RunRequest};
use std::path::Path;
use std::sync::Arc;

/// The task or helper run that owns native observations and accounting.
pub(in crate::engine) struct RunIdentity<'a> {
    pub(in crate::engine) id: &'a str,
    pub(in crate::engine) attempt_n: u32,
    pub(in crate::engine) track: RunTrack,
    pub(in crate::engine) tag: CostTag,
}

/// The provider input and output files of one native invocation.
pub(in crate::engine) struct RunInvocation<'a> {
    pub(in crate::engine) request: &'a RunRequest<'a>,
    pub(in crate::engine) brief: &'a str,
    pub(in crate::engine) events: &'a Path,
    pub(in crate::engine) schema: Option<OutputSchema<'a>>,
}

pub(in crate::engine) async fn run_harness(
    app: &Arc<App>,
    identity: RunIdentity<'_>,
    invocation: RunInvocation<'_>,
    cancel: &CancelToken,
    mut guard: Guard,
) -> Result<RunOutcome, RunError> {
    wait_for_app_secrets(app, identity.id, cancel).await;
    let secrets = run_secrets(app, identity.id).map_err(|error| {
        if let Ok(Some(mut task)) = app.store.load_task(identity.id) {
            task.record_decision(format!("Orchestrator: {error}"));
            let _ = app.store.save_task(&task);
        }
        RunError::Io(error)
    })?;
    let req = invocation.request;
    let started_at = now_ms();
    let native = NativeCommand::new(req, invocation.schema.as_ref());
    let codex_version = match native.version(req.harness, cancel, &mut guard).await {
        Ok(version) => version,
        Err(trip) => {
            let mut outcome = RunOutcome::default();
            trip.apply(&mut outcome);
            record_run(
                app,
                &identity,
                ProviderRun {
                    resume: None,
                    ..ProviderRun::from(req)
                },
                &outcome,
                started_at,
            );
            return Ok(outcome);
        }
    };
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let files = RunFiles::of(invocation.events);
    let tracked = (identity.track != RunTrack::No).then_some(&files);
    let mut prepared = native.prepare(req, tracked, invocation.brief, codex_version);
    prepared.command.current_dir(req.worktree);
    configure_command(app, identity.id, req, &secrets.env, &mut prepared.command);
    if let Some(token) = app.hook_token_of(identity.id, identity.attempt_n) {
        prepared.command.env("SUSHIAI_ORCH_TOKEN", token);
    }
    let spawned = SpawnedRun::spawn(prepared, tracked)?;
    if identity.track != RunTrack::No {
        if let Some(pgid) = spawned.pgid() {
            persist_attempt_observation(
                app,
                identity.id,
                identity.attempt_n,
                crate::task::attempts::RunObservation::ProcessGroup(pgid),
            )
            .await;
        }
    }
    let mut active = ActiveRun {
        process: spawned.attach(tracked, invocation.brief),
        stream: RunStream::default(),
        guard,
        redaction: secrets.redaction,
        started_at,
    };
    active.drive(app, &identity, &invocation, cancel).await?;
    Ok(active.complete(app, &identity, req))
}
