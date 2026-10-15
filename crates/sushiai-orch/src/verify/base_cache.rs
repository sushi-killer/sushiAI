use crate::task::attempt::VerifyOutcome;
use crate::verify::passes::is_interrupted;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

const BASE_RUNS_FILE: &str = "base-results.json";

/// Most results kept on disk; the rest are dropped when the file is written.
const BASE_RUNS_KEPT: usize = 1000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBaseRun {
    base_sha: String,
    outcome: VerifyOutcome,
}

/// A result the environment caused: its setup failed before the command ran.
/// The command's own output never says so (an install error in a check's
/// tail is that check's failure).
pub(crate) fn is_environment_result(outcome: &VerifyOutcome) -> bool {
    outcome.setup_failed
}

/// The base results of an earlier daemon: (base sha, command) -> outcome.
/// A missing or unreadable file is an empty cache; an entry without an exit
/// code or with an interrupted or environment result is skipped.
pub(crate) fn load_base_runs(data_dir: &Path) -> HashMap<(String, String), VerifyOutcome> {
    let rows: Vec<StoredBaseRun> = std::fs::read_to_string(data_dir.join(BASE_RUNS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut rows = rows;
    for row in rows.iter_mut() {
        crate::test_failures::hydrate_lists(data_dir, &mut row.outcome);
    }
    let poisoned = rows
        .iter()
        .any(|r| is_interrupted(&r.outcome) || is_environment_result(&r.outcome));
    let cache = rows
        .into_iter()
        .filter(|r| {
            r.outcome.code.is_some()
                && !is_interrupted(&r.outcome)
                && !is_environment_result(&r.outcome)
        })
        .map(|r| ((r.base_sha, r.outcome.command.clone()), r.outcome))
        .collect();
    if poisoned {
        save_base_runs(data_dir, &cache);
    }
    cache
}

/// Writes the cache next to the other daemon state (best effort).
pub(crate) fn save_base_runs(data_dir: &Path, cache: &HashMap<(String, String), VerifyOutcome>) {
    let rows: Vec<StoredBaseRun> = cache
        .iter()
        .filter(|(_, o)| o.code.is_some() && !is_interrupted(o) && !is_environment_result(o))
        .take(BASE_RUNS_KEPT)
        .map(|((sha, _), o)| StoredBaseRun {
            base_sha: sha.clone(),
            outcome: o.clone(),
        })
        .collect();
    let (path, tmp) = (
        data_dir.join(BASE_RUNS_FILE),
        data_dir.join(format!("{BASE_RUNS_FILE}.tmp")),
    );
    if let Ok(text) = serde_json::to_string(&rows) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests;
