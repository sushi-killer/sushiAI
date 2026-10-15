use crate::task::VerifyOutcome;
use std::path::Path;

const LISTS_DIR: &str = "test-lists";

/// Writes the lists of `outcome` to `data_dir/test-lists/<key>.json` (one file
/// per distinct pair of lists) and sets its counts and `tests_key`. A write
/// that fails leaves the lists in memory and the key empty.
pub fn stash_lists(data_dir: &Path, outcome: &mut VerifyOutcome) {
    outcome.failing_count = outcome.failing.len();
    outcome.passing_count = outcome.passing.len();
    outcome.tests_key = String::new();
    if outcome.failing.is_empty() && outcome.passing.is_empty() {
        return;
    }
    let key = crate::policy::test_output::list_key(&outcome.failing, &outcome.passing);
    let dir = data_dir.join(LISTS_DIR);
    let path = dir.join(format!("{key}.json"));
    let lists = serde_json::json!({"failing": outcome.failing, "passing": outcome.passing});
    if path.exists()
        || (std::fs::create_dir_all(&dir).is_ok()
            && std::fs::write(&path, lists.to_string()).is_ok())
    {
        outcome.tests_key = key;
    }
}

/// Reads the lists `stash_lists` wrote back into `outcome`. Unchanged when the
/// file is missing or does not match the counts: such a run is not comparable.
pub fn hydrate_lists(data_dir: &Path, outcome: &mut VerifyOutcome) {
    if outcome.tests_key.is_empty() || !outcome.failing.is_empty() || !outcome.passing.is_empty() {
        return;
    }
    let lists: Option<serde_json::Value> = std::fs::read_to_string(
        data_dir
            .join(LISTS_DIR)
            .join(format!("{}.json", outcome.tests_key)),
    )
    .ok()
    .and_then(|t| serde_json::from_str(&t).ok());
    let names = |key: &str| -> Option<Vec<String>> {
        serde_json::from_value(lists.as_ref()?.get(key)?.clone()).ok()
    };
    if let (Some(failing), Some(passing)) = (names("failing"), names("passing")) {
        if failing.len() == outcome.failing_count && passing.len() == outcome.passing_count {
            outcome.failing = failing;
            outcome.passing = passing;
        }
    }
}
