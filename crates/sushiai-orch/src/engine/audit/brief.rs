use crate::prompts::brief_text;
use crate::prompts::reply::tagged_json;

/// The fixed rubric a `repo.audit` run grades a repository against. Kept
/// repository-agnostic: it names practices, never one repository's files.
pub fn audit_rubric() -> &'static str {
    brief_text("audit_rubric")
}

/// The most fixes a report keeps, in the order the agent ranked them.
pub const MAX_AUDIT_TOP_FIXES: usize = 5;

/// The `repo.audit` brief: fixed instructions, the rubric verbatim and the
/// report format. It takes no parameters on purpose -- every repository
/// gets the same text.
pub fn build_audit_brief() -> String {
    format!(
        "## Repository audit\n\n{}\n\n## Rubric\n\n{}\n\n{}\n",
        brief_text("audit_instructions"),
        audit_rubric(),
        brief_text("audit_report_format")
    )
}

/// Why a reply yielded no audit report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditParseError {
    /// The reply held no ```sushi-audit block.
    NoBlock,
    /// The block was there but broke the report contract.
    Invalid(String),
}

/// The last ```sushi-audit block (same lookup as the other replies), with
/// grade/effort case folded and topFixes cut to the first
/// `MAX_AUDIT_TOP_FIXES`. Every field is required: no items, an item
/// without evidence or a recommendation, a blank value, an unknown grade or
/// a non-string fix all reject the report -- it is parsed, never filled in.
pub fn parse_audit(text: &str) -> Result<crate::task::audit::AuditReport, AuditParseError> {
    let invalid = |reason: String| AuditParseError::Invalid(reason);
    let body = tagged_json(text, "sushi-audit").ok_or(AuditParseError::NoBlock)?;
    let mut v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| invalid(format!("not JSON: {e}")))?;
    let items = v
        .get_mut("items")
        .and_then(|items| items.as_array_mut())
        .ok_or_else(|| invalid("`items` is missing or not an array".into()))?;
    for item in items.iter_mut() {
        for key in ["grade", "effort"] {
            if let Some(s) = item.get(key).and_then(|x| x.as_str()) {
                item[key] = serde_json::Value::String(s.trim().to_ascii_lowercase());
            }
        }
    }
    if let Some(fixes) = v.get_mut("topFixes").and_then(|f| f.as_array_mut()) {
        fixes.truncate(MAX_AUDIT_TOP_FIXES);
    }
    let report: crate::task::audit::AuditReport =
        serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?;
    if report.summary.trim().is_empty() {
        return Err(invalid("`summary` is blank".into()));
    }
    if report.items.is_empty() {
        return Err(invalid("`items` is empty".into()));
    }
    for (i, item) in report.items.iter().enumerate() {
        if item.area.trim().is_empty() {
            return Err(invalid(format!("item {i} has a blank `area`")));
        }
        if item.recommendation.trim().is_empty() {
            return Err(invalid(format!("item {i} has a blank `recommendation`")));
        }
        if item.evidence.is_empty() || item.evidence.iter().any(|e| e.trim().is_empty()) {
            return Err(invalid(format!("item {i} has no or blank `evidence`")));
        }
    }
    if report.top_fixes.iter().any(|fix| fix.trim().is_empty()) {
        return Err(invalid("`topFixes` has a blank entry".into()));
    }
    Ok(report)
}

#[cfg(test)]
#[path = "tests/brief.rs"]
mod tests;
