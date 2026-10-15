use super::claude::feed_claude_line;
use super::codex::feed_codex_line;
use super::outcome::RunOutcome;
use crate::policy::routing::Harness;

/// Folds a whole saved `events.jsonl` (stderr lines included; they are
/// skipped) into an outcome, as if streamed.
pub fn replay_events(harness: Harness, text: &str) -> RunOutcome {
    let mut outcome = RunOutcome::default();
    for line in text.lines() {
        feed_stream_line(harness, line, &mut outcome);
    }
    outcome
}

/// Folds one line of a harness's streamed JSON output into `outcome`,
/// returning a short human-readable note for the `log` subscribe event when
/// the line is worth surfacing (at most one line per event, per spec).
/// Unparseable or irrelevant lines are ignored, never fatal -- a harness
/// version bump changing an unrelated field must not break the run.
pub fn feed_stream_line(harness: Harness, line: &str, outcome: &mut RunOutcome) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match harness {
        Harness::Claude => feed_claude_line(&v, outcome),
        Harness::Codex => feed_codex_line(&v, outcome),
    }
}

pub(super) fn short_note(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line.to_string()
    }
}
