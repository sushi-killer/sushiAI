use crate::harness::native::error::RunError;
use crate::harness::{replay_events, RunOutcome};
use crate::task::attempt::Attempt;
use std::path::{Path, PathBuf};

pub(super) fn events(run_dir: &Path, attempt: &Attempt) -> Option<PathBuf> {
    attempt
        .candidates
        .get(1)
        .filter(|candidate| candidate.picked)
        .map(|_| run_dir.join("b/events.jsonl"))
}

pub(super) fn replay(events: &Path, attempt: &Attempt) -> Result<RunOutcome, RunError> {
    let text = std::fs::read_to_string(events).map_err(|error| RunError::Io(error.to_string()))?;
    let mut outcome = replay_events(attempt.harness, &text);
    apply_receipt(attempt, &mut outcome);
    Ok(outcome)
}

pub(super) fn restore_receipt(attempt: &Attempt, outcome: &mut RunOutcome) -> bool {
    if attempt.fingerprint.is_none() || attempt.usage.is_none() {
        return false;
    }
    apply_receipt(attempt, outcome);
    true
}

fn apply_receipt(attempt: &Attempt, outcome: &mut RunOutcome) {
    outcome.session_id = attempt.session_id.clone();
    outcome.fingerprint = attempt.fingerprint.clone();
    outcome.cost_usd = attempt.cost_usd.or_else(|| {
        attempt
            .candidates
            .iter()
            .find(|candidate| candidate.picked)
            .map(|candidate| candidate.cost)
    });
    outcome.cost_estimated = attempt.cost_estimated;
    outcome.first_turn_tokens = attempt.prefix_tokens;
    if let Some(usage) = &attempt.usage {
        outcome.usage_input = usage.input;
        outcome.usage_output = usage.output;
        outcome.usage_cached = usage.cached;
    }
}
