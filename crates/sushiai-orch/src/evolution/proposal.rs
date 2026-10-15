use crate::prompts::reply::tagged_json;
use crate::prompts::{brief_text, fill};

/// Occurrences quoted into a proposer brief, and lines quoted per occurrence.
pub const MAX_PROPOSAL_OCCURRENCES: usize = 5;

pub const MAX_PROPOSAL_EXCERPT_LINES: usize = 40;

/// Raw `events.jsonl` lines around one signal's `excerptRef`.
pub struct ProposalExcerpt {
    pub task_id: String,
    pub file: String,
    /// 1-based line number of `lines[0]` in `file`.
    pub from_line: usize,
    pub lines: Vec<String>,
}

/// What the proposer brief says about one cluster of signals.
pub struct ProposalBriefInput<'a> {
    pub kind: &'a str,
    pub detail: &'a str,
    pub repo: &'a str,
    pub occurrences: usize,
    pub tasks: usize,
    pub wasted_calls: u32,
    pub wasted_usd: f64,
    pub excerpts: &'a [ProposalExcerpt],
}

/// The evolution proposer brief: fixed instructions, the cluster's numbers,
/// the raw event lines (never a summary of them) and the reply format.
pub fn build_proposal_brief(input: &ProposalBriefInput) -> String {
    let cluster = fill(
        brief_text("proposal_cluster"),
        &[
            ("kind", input.kind),
            ("detail", input.detail),
            ("repo", input.repo),
            ("occurrences", &input.occurrences.to_string()),
            ("tasks", &input.tasks.to_string()),
            ("wasted_calls", &input.wasted_calls.to_string()),
            ("wasted_usd", &format!("{:.2}", input.wasted_usd)),
        ],
    );
    let mut out = format!(
        "## Evolution proposal\n\n{}\n\n{cluster}\n",
        brief_text("proposal_instructions")
    );
    for (i, excerpt) in input
        .excerpts
        .iter()
        .take(MAX_PROPOSAL_OCCURRENCES)
        .enumerate()
    {
        let lines: Vec<&String> = excerpt
            .lines
            .iter()
            .take(MAX_PROPOSAL_EXCERPT_LINES)
            .collect();
        let to = excerpt.from_line + lines.len().saturating_sub(1);
        out.push_str(&format!(
            "\n### Occurrence {}: task {}, {} lines {}-{}\n\n",
            i + 1,
            excerpt.task_id,
            excerpt.file,
            excerpt.from_line,
            to
        ));
        // A fence longer than any backtick run in the lines, so raw content
        // can never close it early.
        let longest_run = lines
            .iter()
            .flat_map(|l| l.split(|c| c != '`'))
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat((longest_run + 1).max(3));
        out.push_str(&fence);
        out.push_str("text\n");
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&fence);
        out.push('\n');
    }
    if input.excerpts.is_empty() {
        out.push_str("\n(no excerpt could be read)\n");
    }
    out.push('\n');
    out.push_str(brief_text("proposal_report_format"));
    out.push('\n');
    out
}

/// Why a reply yielded no proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalParseError {
    /// The reply held no ```sushi-proposal block.
    NoBlock,
    /// The block was there but broke the proposal contract.
    Invalid(String),
}

/// The last ```sushi-proposal block, with `track` and `form` case folded.
/// Every field but `arm` is required and non-blank; `arm` must be an object.
pub fn parse_proposal(
    text: &str,
) -> Result<crate::task::evolution::ProposalReply, ProposalParseError> {
    let invalid = ProposalParseError::Invalid;
    let body = tagged_json(text, "sushi-proposal").ok_or(ProposalParseError::NoBlock)?;
    let mut v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| invalid(format!("not JSON: {e}")))?;
    for key in ["track", "form"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            v[key] = serde_json::Value::String(s.trim().to_ascii_lowercase());
        }
    }
    let reply: crate::task::evolution::ProposalReply =
        serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?;
    for (name, value) in [
        ("change", &reply.change),
        ("evidence", &reply.evidence),
        ("metric", &reply.metric),
        ("test", &reply.test),
    ] {
        if value.trim().is_empty() {
            return Err(invalid(format!("`{name}` is blank")));
        }
    }
    if matches!(&reply.arm, Some(arm) if !arm.is_object()) {
        return Err(invalid("`arm` is not an object".into()));
    }
    Ok(reply)
}

#[cfg(test)]
#[path = "tests/proposal.rs"]
mod tests;
