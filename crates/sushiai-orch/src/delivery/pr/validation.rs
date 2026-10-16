use crate::task::Task;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const HEADING: &str = "## Validation";

/// Check observations retain their Git tree; a commit is named only for that tree.
pub(crate) fn section(task: &Task, commits: &BTreeMap<String, String>) -> String {
    let mut out = format!("\n{HEADING}\n\n");
    let mut seen = BTreeSet::new();
    for result in task
        .attempts
        .iter()
        .flat_map(|attempt| &attempt.verify)
        .chain(&task.delivery_checks)
    {
        let identity = (&result.command, &result.commit, &result.tree, result.code);
        if !seen.insert(identity) {
            continue;
        }
        let location = match (&result.commit, &result.tree) {
            (Some(commit), Some(tree)) => format!("checkout commit `{commit}`; checked tree `{tree}`"),
            (Some(commit), None) => format!("checkout commit `{commit}`; checked tree not recorded"),
            (None, Some(tree)) => match commits.get(tree) {
                Some(commit) => format!("checked tree `{tree}`; same tree as commit `{commit}` (original checkout commit not recorded)"),
                None => format!("checked tree `{tree}` (checkout commit not recorded)"),
            },
            (None, None) => "checkout commit and tree not recorded".to_string(),
        };
        let status = result.code.map_or_else(
            || "interrupted or no exit code".to_string(),
            |code| {
                if code == 0 {
                    "passed".into()
                } else {
                    format!("failed (exit {code})")
                }
            },
        );
        out.push_str(&format!("- `{}`: {status}; {location}.\n", result.command));
    }
    if seen.is_empty() {
        out.push_str("No check results were recorded.\n");
    }
    out
}

/// Drafted check sections are replaced by the recorded observations.
pub(crate) fn without_section(body: &str) -> String {
    let mut skipping = false;
    let mut kept = Vec::new();
    for line in body.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            skipping = matches!(
                title.trim().to_ascii_lowercase().as_str(),
                "validation" | "verification" | "testing" | "tests" | "test plan" | "checks"
            );
        }
        if !skipping {
            kept.push(line);
        }
    }
    kept.join("\n")
}

#[cfg(test)]
mod tests;
