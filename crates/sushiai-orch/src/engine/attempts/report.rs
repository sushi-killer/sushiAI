use crate::prompts::reply::{last_fenced_block, lenient_text, tagged_json};
use crate::task::questions::Risk;
use crate::task::review::Decline;

/// The implementer's `sushi-impossible` claim about one criterion.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Impossible {
    pub criterion: usize,
    #[serde(default)]
    pub evidence: String,
}

/// The last fenced `sushi-impossible` block, when it names a criterion in
/// range. Fenced only: the outermost-`{...}` fallback other tags get would
/// match the `sushi-report` JSON.
pub fn parse_impossible(text: &str, criteria_len: usize) -> Option<Impossible> {
    let body = last_fenced_block(text, "sushi-impossible")?;
    serde_json::from_str::<Impossible>(&body)
        .ok()
        .filter(|i| i.criterion < criteria_len)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Complete,
    Partial,
    Blocked,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Report {
    pub outcome: Outcome,
    #[serde(default)]
    pub summary: String,
    #[serde(default, deserialize_with = "lenient_text")]
    pub handoff: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub question: String,
    /// What the blocked `question` commits the owner to (`Question.risk`).
    #[serde(default)]
    pub risk: Vec<Risk>,
    #[serde(default, deserialize_with = "lenient_declines")]
    pub declines: Vec<Decline>,
    /// Protected tests the agent says are wrong, with the reason.
    #[serde(default, rename = "testClaims", deserialize_with = "lenient_claims")]
    pub test_claims: Vec<TestClaim>,
}

/// "This protected test is wrong", from the implementer's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestClaim {
    pub path: String,
    pub reason: String,
}

/// Entries without a path and a reason are dropped; anything but an array is
/// empty.
fn lenient_claims<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<TestClaim>, D::Error> {
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    let serde_json::Value::Array(entries) = v else {
        return Ok(Vec::new());
    };
    let text = |e: &serde_json::Value, key: &str| {
        e.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Ok(entries
        .iter()
        .filter_map(|e| {
            Some(TestClaim {
                path: text(e, "path")?,
                reason: text(e, "reason")?,
            })
        })
        .collect())
}

/// Declines of review findings. An entry is refused unless it parses as a
/// `Decline` (an id, a known reason and an `evidence` array) and at least
/// one piece of its evidence is not blank. Anything but an array is empty.
fn lenient_declines<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Decline>, D::Error> {
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    let serde_json::Value::Array(entries) = v else {
        return Ok(Vec::new());
    };
    Ok(entries
        .into_iter()
        .filter_map(|e| serde_json::from_value::<Decline>(e).ok())
        .map(|d| Decline {
            id: d.id.trim().to_string(),
            ..d
        })
        .filter(|d| !d.id.is_empty() && d.evidence.iter().any(|e| !e.trim().is_empty()))
        .collect())
}

/// Extract the JSON body of the *last* ```sushi-report fenced block in
/// `text`. Missing or malformed -> `None` (spec: "last fence wins, malformed
/// -> None").
pub fn parse_report(text: &str) -> Option<Report> {
    serde_json::from_str(&tagged_json(text, "sushi-report")?).ok()
}

/// The attempt's own report decisions as `Agent: ...` lines: a leading
/// `Owner:`/`Lead:`/`Agent:` the agent echoed is stripped before the prefix is added.
pub fn agent_decision_lines(decisions: &[String]) -> Vec<String> {
    decisions
        .iter()
        .map(|d| {
            let cleaned = ["Owner:", "Lead:", "Agent:"]
                .iter()
                .find_map(|p| d.strip_prefix(p))
                .map(|s| s.trim_start())
                .unwrap_or(d.as_str());
            format!("Agent: {cleaned}")
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/report.rs"]
mod tests;
