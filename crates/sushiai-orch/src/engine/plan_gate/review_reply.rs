use super::MAX_FINDING_CHARS;
use crate::policy::settings::Settings;
use crate::prompts::clip::truncate_chars;
use crate::task::{PlanFinding, Severity, Task};
use serde::Deserialize;

/// The review's kinds of finding. The schema is the contract: a reply that
/// does not match it is an unusable reply, never read leniently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ReviewKind {
    MissingRequirement,
    ReportOnly,
    Untestable,
    WrongSplit,
    WrongApproach,
}

impl ReviewKind {
    fn label(self) -> &'static str {
        match self {
            ReviewKind::MissingRequirement => "missing-requirement",
            ReviewKind::ReportOnly => "report-only",
            ReviewKind::Untestable => "untestable",
            ReviewKind::WrongSplit => "wrong-split",
            ReviewKind::WrongApproach => "wrong-approach",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReviewSeverity {
    Critical,
    High,
    Medium,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ReviewFinding {
    kind: ReviewKind,
    severity: ReviewSeverity,
    text: String,
    criterion: Option<usize>,
    request_item: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewReply {
    findings: Vec<ReviewFinding>,
}

/// Reads the review's reply: the first `{` to the last `}` is the JSON, serde
/// decides the rest. A criterion index the draft does not have is unusable too.
pub(super) fn parse_review(text: &str, criteria: usize) -> Result<Vec<ReviewFinding>, String> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("the reply had no JSON".to_string());
    };
    let reply: ReviewReply = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("the reply did not match the schema: {e}"))?;
    for (n, f) in reply.findings.iter().enumerate() {
        match f.criterion {
            Some(i) if i >= criteria => {
                return Err(format!(
                    "finding {n} names criterion {i}, but the draft has {criteria}"
                ))
            }
            None if f.kind == ReviewKind::ReportOnly => {
                return Err(format!("finding {n} is report-only but names no criterion"))
            }
            _ => {}
        }
    }
    Ok(reply.findings)
}

/// The stored form of one review finding; `n` numbers the stored ones.
fn plan_finding(n: usize, f: &ReviewFinding) -> PlanFinding {
    let body = f.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = match &f.request_item {
        Some(item) => format!("\"{}\": {body}", truncate_chars(item.trim(), 160)),
        None => body,
    };
    PlanFinding {
        id: format!("review-{n}"),
        severity: match f.severity {
            ReviewSeverity::Critical => Severity::Critical,
            ReviewSeverity::High => Severity::High,
            ReviewSeverity::Medium => Severity::Medium,
        },
        kind: f.kind.label().to_string(),
        text: truncate_chars(&text, MAX_FINDING_CHARS),
    }
}

/// The criteria a typed report-only finding names. Nothing else removes one.
fn report_only_criteria(found: &[ReviewFinding]) -> Vec<usize> {
    let mut out: Vec<usize> = found
        .iter()
        .filter(|f| f.kind == ReviewKind::ReportOnly)
        .filter_map(|f| f.criterion)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The criteria the review may remove and the findings the plan keeps. A
/// report-only criterion that carries a shared check is never removed here
/// (only the owner drops one): its finding stays, never above `High`.
pub(super) fn split_review(
    settings: &Settings,
    task: &Task,
    found: &[ReviewFinding],
) -> (Vec<usize>, Vec<PlanFinding>) {
    let (removals, shared): (Vec<usize>, Vec<usize>) =
        report_only_criteria(found).into_iter().partition(|&i| {
            let commands = crate::task::checks::authority::criterion_commands(task, i);
            crate::engine::answer_policy::authority::removal_refusal(
                settings,
                task,
                &commands,
                crate::task::checks::authority::Origin::Other,
            )
            .is_none()
        });
    let review = found
        .iter()
        .filter(|f| {
            f.kind != ReviewKind::ReportOnly || f.criterion.is_some_and(|i| shared.contains(&i))
        })
        .enumerate()
        .map(|(n, f)| {
            let mut finding = plan_finding(n, f);
            if f.kind == ReviewKind::ReportOnly && finding.severity == Severity::Critical {
                finding.severity = Severity::High;
            }
            finding
        })
        .collect();
    (removals, review)
}

#[cfg(test)]
mod tests;
