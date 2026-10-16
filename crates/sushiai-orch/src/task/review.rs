use serde::{Deserialize, Serialize};

/// Why the implementer declines a review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclineReason {
    Wrong,
    AlreadySatisfied,
    ChangesBehaviour,
}

/// A review finding the implementer declines, with its evidence. The next
/// review rules on it by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decline {
    pub id: String,
    pub reason: DeclineReason,
    pub evidence: Vec<String>,
}

/// The next review's ruling on one decline: an upheld decline resolves its
/// finding; otherwise the finding stays blocking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclineRuling {
    pub id: String,
    pub upheld: bool,
}

/// How much of an attempt a review looked at: everything, or only the open
/// findings of the review before it and the task's changes since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewScope {
    Full,
    Narrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Fail,
}

/// Whether the latest review still reports a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    Open,
    Resolved,
}

/// One review finding of a task, kept under its stable id (`F-` and six hex
/// characters of the finding's key hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRecord {
    pub id: String,
    pub text: String,
    pub status: FindingStatus,
    /// The attempt and review round that last saw it.
    pub attempt: u32,
    pub round: u32,
    /// How many later attempts' reviews reported it again.
    #[serde(default)]
    pub repeats: u32,
    /// The `scenario.run` commands reviewers gave for it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenarios: Vec<String>,
    /// Its second repeat was turned into a check or sent to the judge.
    #[serde(default, skip_serializing_if = "is_false")]
    pub handled: bool,
}

/// How a reviewer shows a finding fails: a command and what it prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingScenario {
    #[serde(default)]
    pub run: String,
    #[serde(default)]
    pub fails: String,
}

/// The optional marks of one finding: `id` of a registered finding it is
/// about, `caused_by_diff` (missing counts as true), `kind` and `scenario`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by_diff: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<FindingScenario>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResult {
    pub verdict: Verdict,
    #[serde(default)]
    pub findings: Vec<String>,
    /// The findings the registry already held open from an earlier attempt:
    /// still present after that attempt's review reported them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repeated: Vec<String>,
    /// The severity (0-3) of each finding by position, `None` when it named
    /// none; findings past the end (criterion rulings) have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub severities: Vec<Option<u8>>,
    /// The `kind` and `scenario` of each of the reviewer's own findings by
    /// position; criterion findings past them have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<FindingDetail>,
    /// The reviewer's ruling on each criterion it named, `met: true` too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<CriterionRuling>,
}

/// One criterion ruling of a review: `met` is `None` when the reviewer could
/// not check it. `index` is the criterion's position in the task, `None`
/// when the reviewer named none the task has; `criterion` is its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionRuling {
    pub criterion: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default)]
    pub met: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// A leading P0-P3 token (optionally bracketed) followed by a
/// non-alphanumeric character or the end of the text.
pub fn severity_of(text: &str) -> Option<u8> {
    let t = text.trim_start();
    let t = t.strip_prefix('[').unwrap_or(t);
    let mut chars = t.chars();
    if !matches!(chars.next(), Some('P' | 'p')) {
        return None;
    }
    let level = chars.next()?.to_digit(10).filter(|d| *d <= 3)? as u8;
    match chars.next() {
        Some(c) if c.is_alphanumeric() => None,
        _ => Some(level),
    }
}
