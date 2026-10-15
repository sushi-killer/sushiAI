use super::MAX_FINDING_CHARS;
use crate::prompts::clip::truncate_chars;
use crate::task::{PlanFinding, Severity};

/// Sections orchd appends to a subtask's request; they are not the owner's
/// own text, so no enumerated item of theirs counts.
const APPENDED_MARKERS: [&str; 2] = [
    "This is one part of a larger task",
    "Already decided for the whole task",
];

fn finding(kind: &str, n: usize, severity: Severity, text: String) -> PlanFinding {
    PlanFinding {
        id: format!("{kind}-{n}"),
        severity,
        kind: kind.to_string(),
        text: truncate_chars(&text, MAX_FINDING_CHARS),
    }
}

/// The owner's own words: the request up to the first section orchd added.
pub(in crate::engine::plan_gate) fn owner_text(request: &str) -> &str {
    let end = APPENDED_MARKERS
        .iter()
        .filter_map(|m| request.find(m))
        .min()
        .unwrap_or(request.len());
    &request[..end]
}

/// A backticked span of the owner's text that is a shared check.
fn named_shared_checks(request: &str, shared: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, span) in owner_text(request).split('`').enumerate() {
        let span = span.split_whitespace().collect::<Vec<_>>().join(" ");
        if i % 2 == 1 && !span.is_empty() && shared(&span) && !out.contains(&span) {
            out.push(span);
        }
    }
    out
}

fn flat(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The shared checks the request names must still run. An exact-string check
/// is the one code check the gate keeps: a dropped shared check is critical.
pub(super) fn shared_check_findings(
    request: &str,
    verify: &[String],
    final_verify: &[String],
    shared: &dyn Fn(&str) -> bool,
) -> Vec<PlanFinding> {
    let mut out = Vec::new();
    for (n, command) in named_shared_checks(request, shared).iter().enumerate() {
        let wanted = flat(command);
        let kept = verify
            .iter()
            .chain(final_verify)
            .any(|c| flat(c).contains(&wanted));
        if !kept {
            out.push(finding(
                "shared-check",
                n,
                Severity::Critical,
                format!(
                    "The request names the shared check `{command}` but neither verify nor finalVerify runs it"
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;
