use crate::clock::now_ms;
use crate::engine::attempts::configure::configure_command;
use crate::engine::attempts::ledger::record_run;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::pricing::guard_line;
use crate::engine::attempts::secrets::run_secrets;
use crate::engine::attempts::secrets::wait_for_app_secrets;
use crate::engine::attempts::settle::cancel_run;
use crate::engine::attempts::settle::complete_run;
use crate::engine::attempts::stream::RunStream;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::persistence::persist_attempt_observation;
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::harness::native::io::NativeRun;
use crate::harness::native::io::ProcessEvent;
use crate::harness::native::io::SpawnedRun;
use crate::harness::native::launch::NativeCommand;
use crate::harness::native::redaction::redact;
use crate::harness::run_files::RunFiles;
use crate::store::log::append_line;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn run_harness(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    tag: CostTag,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    guard: Guard,
) -> Result<harness::RunOutcome, RunError> {
    run_harness_schema(
        app,
        task_id,
        attempt_n,
        track,
        worktree,
        req,
        tag,
        brief_text,
        events_path,
        cancel,
        guard,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn run_harness_schema(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    tag: CostTag,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    mut guard: Guard,
    schema: Option<&harness::OutputSchema<'_>>,
) -> Result<harness::RunOutcome, RunError> {
    wait_for_app_secrets(app, task_id, cancel).await;
    let secrets = run_secrets(app, task_id).map_err(|error| {
        if let Ok(Some(mut task)) = app.store.load_task(task_id) {
            task.record_decision(format!("Orchestrator: {error}"));
            let _ = app.store.save_task(&task);
        }
        RunError::Io(error)
    })?;
    let started_at = now_ms();
    let native = NativeCommand::new(req, schema);
    let codex_version = match native.version(req.harness, cancel, &mut guard).await {
        Ok(version) => version,
        Err(trip) => {
            let mut outcome = harness::RunOutcome::default();
            trip.apply(&mut outcome);
            record_run(
                app,
                task_id,
                attempt_n,
                &tag,
                (req.harness, req.model, false),
                &outcome,
                started_at,
            );
            return Ok(outcome);
        }
    };
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let files = RunFiles::of(events_path);
    let tracked = (track != RunTrack::No).then_some(&files);
    let mut prepared = native.prepare(req, tracked, brief_text, codex_version);
    prepared.command.current_dir(worktree);
    configure_command(app, task_id, req, &secrets.env, &mut prepared.command);
    if let Some(token) = app.hook_token_of(task_id, attempt_n) {
        prepared.command.env("SUSHIAI_ORCH_TOKEN", token);
    }
    let spawned = SpawnedRun::spawn(prepared, tracked)?;
    if track != RunTrack::No {
        if let Some(pgid) = spawned.pgid() {
            persist_attempt_observation(
                app,
                task_id,
                attempt_n,
                crate::task::attempts::RunObservation::ProcessGroup(pgid),
            )
            .await;
        }
    }
    let mut process = spawned.attach(tracked, brief_text);
    let stream = drive_run(
        app,
        task_id,
        attempt_n,
        track,
        req,
        &tag,
        events_path,
        cancel,
        &mut guard,
        &mut process,
        &secrets.redaction,
        started_at,
    )
    .await?;
    Ok(complete_run(
        app,
        task_id,
        attempt_n,
        track,
        req,
        &tag,
        &process,
        stream.outcome,
        started_at,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn drive_run(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    req: &harness::RunRequest<'_>,
    tag: &CostTag,
    events_path: &Path,
    cancel: &CancelToken,
    guard: &mut Guard,
    process: &mut NativeRun,
    redaction: &HashMap<String, String>,
    started_at: i64,
) -> Result<RunStream, RunError> {
    let mut stream = RunStream::default();
    loop {
        match process.next(cancel, guard).await {
            ProcessEvent::Cancelled => {
                return Err(cancel_run(
                    app,
                    task_id,
                    attempt_n,
                    track,
                    req,
                    tag,
                    events_path,
                    process,
                    &stream.outcome,
                    started_at,
                )
                .await);
            }
            ProcessEvent::Tripped(trip) => {
                process.stop().await;
                trip.apply(&mut stream.outcome);
                break;
            }
            ProcessEvent::Stdout(line) => {
                let line = redact(&line, redaction);
                append_line(events_path, &line);
                if let Some(note) =
                    harness::feed_stream_line(req.harness, &line, &mut stream.outcome)
                {
                    app.broadcast_log(task_id, attempt_n, note);
                }
                if let Some(trip) =
                    guard_line(app, guard, (req.harness, req.model), &line, &stream.outcome)
                {
                    process.stop().await;
                    trip.apply(&mut stream.outcome);
                    break;
                }
                if track != RunTrack::No {
                    stream.observe_attempt(app, task_id, attempt_n, &line).await;
                }
            }
            ProcessEvent::Stderr(line) => {
                guard.heard();
                let line = redact(&line, redaction);
                append_line(events_path, &format!("[stderr] {line}"));
                stream.stderr(&line);
            }
            ProcessEvent::Exit(status) => {
                stream.exit(req.harness, track != RunTrack::No, status)?;
                break;
            }
        }
    }
    Ok(stream)
}
