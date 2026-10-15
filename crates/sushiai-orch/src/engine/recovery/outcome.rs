use crate::engine::attempts::ledger::{record_run, CostTag};
use crate::engine::attempts::pricing::{finalize_cost, price_resumed_attempt};
use crate::engine::attempts::run::RunIdentity;
use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::Trip;
use crate::harness::native::error::RunError;
use crate::harness::run_files::RunFiles;
use crate::policy::routing::Harness;
use crate::task::attempt::Attempt;
use crate::task::state::Task;

pub(super) fn settle(
    app: &App,
    task: &Task,
    attempt: &Attempt,
    files: &RunFiles,
    trip: Option<Trip>,
) -> Result<harness::RunOutcome, RunError> {
    let n = attempt.n;
    let model = (!attempt.model.is_empty()).then(|| attempt.model.clone());
    let code = std::fs::read_to_string(&files.exit)
        .ok()
        .and_then(|t| t.trim().parse::<i32>().ok())
        // A group killed hard before its wrapper wrote the code.
        .or(trip.as_ref().map(|_| 137));
    if code.is_none() {
        return Err(RunError::Io(
            "The run ended without leaving an exit code.".into(),
        ));
    }
    let raw = std::fs::read_to_string(&files.raw).unwrap_or_default();
    let mut outcome = harness::replay_events(attempt.harness, &raw);
    if let Some(trip) = trip {
        trip.apply(&mut outcome);
    }
    if code != Some(0) && outcome.error.is_none() && outcome.looped.is_none() {
        let stderr = std::fs::read_to_string(&files.stderr).unwrap_or_default();
        let tail: String = stderr.trim().chars().rev().take(4000).collect();
        let tail: String = tail.chars().rev().collect();
        outcome.error = Some(if !tail.is_empty() {
            tail
        } else {
            use std::os::unix::process::ExitStatusExt;
            harness::exit_error(
                attempt.harness,
                std::process::ExitStatus::from_raw(code.unwrap_or(1) << 8),
            )
        });
    }
    if super::selected::restore_receipt(attempt, &mut outcome) {
        return Ok(outcome);
    }
    finalize_cost(app, attempt.harness, model.as_deref(), &mut outcome);
    let meta = RunMetadata::read(files);
    let resume = meta.resume(attempt.harness);
    price_resumed_attempt(
        app,
        &task.id,
        n,
        crate::engine::attempts::pricing::ProviderRun {
            harness: attempt.harness,
            model: model.as_deref(),
            resume,
        },
        &mut outcome,
    );
    outcome.fingerprint = Some(outcome.build_fingerprint(
        attempt.harness,
        meta.codex_version.clone(),
        harness::prompt_hash(&meta.argv, None),
    ));
    // A run the earlier daemon already settled (it died during review) has its record.
    if attempt.fingerprint.is_none() {
        record_run(
            app,
            &RunIdentity {
                id: &task.id,
                attempt_n: n,
                track: crate::engine::attempts::tracking::RunTrack::Resumable,
                tag: CostTag::task("implement", &attempt.route_id),
            },
            crate::engine::attempts::pricing::ProviderRun {
                harness: attempt.harness,
                model: model.as_deref(),
                resume,
            },
            &outcome,
            attempt.started_at,
        );
    }
    Ok(outcome)
}

struct RunMetadata {
    argv: Vec<String>,
    codex_version: Option<String>,
}

impl RunMetadata {
    fn read(files: &RunFiles) -> Self {
        let meta: serde_json::Value = std::fs::read_to_string(&files.meta)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            argv: meta
                .get("argv")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default(),
            codex_version: meta
                .get("codexVersion")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        }
    }

    fn resume(&self, harness: Harness) -> Option<&str> {
        match harness {
            Harness::Claude => self
                .argv
                .windows(2)
                .find(|words| words[0] == "--resume")
                .map(|words| words[1].as_str()),
            Harness::Codex => self
                .argv
                .get(2)
                .filter(|_| self.argv.get(1).is_some_and(|arg| arg == "resume"))
                .map(String::as_str),
        }
    }
}
