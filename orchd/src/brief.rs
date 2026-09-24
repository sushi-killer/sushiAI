//! Harness-agnostic brief text and the fenced report/review the agent hands
//! back. Deliberately produces plain markdown with no Claude- or
//! Codex-specific wording, so the same text is sent on stdin to either
//! harness (spec step 3).

use crate::model::{Attempt, AttemptStatus, Failure, ReviewResult, Task};

const MAX_FAILURE_DETAIL: usize = 1500;

/// Truncate to at most `max` bytes on a char boundary, so we never split a
/// multi-byte UTF-8 sequence.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\u{2026}", &s[..end])
}

fn attempt_outcome_label(attempt: &Attempt) -> &'static str {
    match attempt.status {
        AttemptStatus::Passed => "passed",
        AttemptStatus::Failed => "failed",
        AttemptStatus::Interrupted => "interrupted",
        AttemptStatus::Blocked => "blocked",
        AttemptStatus::Running => "running",
    }
}

const REPORT_FORMAT_BLOCK: &str = "## Report format\n\nEnd your final message with:\n\n```sushi-report\n{\"outcome\":\"complete|partial|blocked\",\"summary\":\"...\",\"decisions\":[],\"question\":\"\"}\n```\n";

const RULES_BLOCK: &str = "## Rules\n\n- Work only in this directory; do not touch other checkouts.\n- Do not commit, push or open pull requests: the orchestrator runs the verification commands and commits after you finish. Project instructions about committing, PRs, release notes or review loops do not apply inside this task.\n- Run the verification commands yourself before finishing. If a command is denied or unavailable, continue without it and say so in your report; that is not a reason to stop.\n- Report `blocked` only for a decision the task and repository cannot answer; anything you can look up, decide it yourself and record it in `decisions`.\n";

/// Full brief for a fresh (non-resumed) attempt.
pub fn build_brief(task: &Task, git_status_short: &str, git_diff_stat: &str) -> String {
    let mut out = String::new();

    out.push_str("## Task\n\n");
    out.push_str(&task.title);
    out.push_str("\n\n");
    out.push_str(&task.goal);
    out.push_str("\n\n");

    out.push_str("## Acceptance criteria\n\n");
    if task.criteria.is_empty() {
        out.push_str("(none specified)\n");
    } else {
        for c in &task.criteria {
            out.push_str("- ");
            out.push_str(c);
            out.push('\n');
        }
    }
    out.push('\n');

    out.push_str("## Verification commands\n\n");
    if task.verify.is_empty() {
        out.push_str("(none specified)\n");
    } else {
        for v in &task.verify {
            out.push_str("- `");
            out.push_str(v);
            out.push_str("`\n");
        }
    }
    out.push('\n');

    out.push_str(RULES_BLOCK);
    out.push('\n');

    if !task.decisions.is_empty() {
        out.push_str("## Decisions and owner answers\n\n");
        for d in &task.decisions {
            out.push_str("- ");
            out.push_str(d);
            out.push('\n');
        }
        out.push('\n');
    }

    if !task.attempts.is_empty() {
        out.push_str("## Previous attempts\n\n");
        for attempt in &task.attempts {
            out.push_str(&format!(
                "- Attempt {} ({}): {}\n",
                attempt.n,
                attempt.route_id,
                attempt_outcome_label(attempt)
            ));
            if let Some(failure) = &attempt.failure {
                let detail = truncate_chars(&failure.detail, MAX_FAILURE_DETAIL);
                out.push_str("  - failure: ");
                out.push_str(&detail);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    out.push_str("## Current state\n\n");
    out.push_str("`git status --short`:\n```\n");
    out.push_str(git_status_short.trim_end());
    out.push_str("\n```\n\n");
    out.push_str("`git diff --stat` (against base):\n```\n");
    out.push_str(git_diff_stat.trim_end());
    out.push_str("\n```\n\n");

    out.push_str(REPORT_FORMAT_BLOCK);

    out
}

/// A resumed session only needs the delta: the failure that ended the
/// previous attempt, and a reminder of the report format (spec step 3,
/// "A resumed session gets only the delta").
pub fn build_resume_delta(failure: &Failure) -> String {
    let mut out = String::new();
    out.push_str("## Previous attempt failed\n\n");
    out.push_str(&truncate_chars(&failure.detail, MAX_FAILURE_DETAIL));
    out.push_str("\n\n");
    out.push_str(REPORT_FORMAT_BLOCK);
    out
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
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub question: String,
}

/// Extract the JSON body of the *last* ```sushi-report fenced block in
/// `text`. Missing or malformed -> `None` (spec: "last fence wins, malformed
/// -> None").
pub fn parse_report(text: &str) -> Option<Report> {
    let body = last_fenced_block(text, "sushi-report")?;
    serde_json::from_str(&body).ok()
}

/// Same idea for the review's ```sushi-review fenced JSON.
pub fn parse_review(text: &str) -> Option<ReviewResult> {
    let body = last_fenced_block(text, "sushi-review")?;
    serde_json::from_str(&body).ok()
}

/// Find the last occurrence of a fence opened with ` ```<tag>` (optionally
/// followed by more characters on the same line, e.g. trailing whitespace)
/// and closed by the next ` ``` ` on its own line, returning the text
/// between them.
fn last_fenced_block(text: &str, tag: &str) -> Option<String> {
    let open_marker = format!("```{}", tag);
    let start_of_open = text.rfind(&open_marker)?;
    let after_open = start_of_open + open_marker.len();
    // Skip to the end of the opening fence's line.
    let content_start = match text[after_open..].find('\n') {
        Some(nl) => after_open + nl + 1,
        None => return None, // opening fence with nothing after it
    };
    let close_offset = text[content_start..].find("```")?;
    let body = &text[content_start..content_start + close_offset];
    Some(body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FailureKind, Harness, Stage, TaskStatus, Tier, Verdict};

    fn sample_task() -> Task {
        Task {
            id: "t1".into(),
            title: "Add a button".into(),
            goal: "Add a Save button to the settings dialog".into(),
            criteria: vec!["Button visible".into(), "Click saves settings".into()],
            verify: vec!["npm test".into()],
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/add-a-button".into(),
            base_sha: "abc123".into(),
            status: TaskStatus::Running,
            tier: Tier::Standard,
            question: None,
            decisions: vec!["Owner: use the primary button style".into()],
            attempts: vec![],
            cost_usd: 0.0,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn brief_is_identical_regardless_of_which_harness_will_run_it() {
        // build_brief takes no harness parameter at all: the same task and
        // git state always produce the same text, whether the caller is
        // about to hand it to Claude or to Codex.
        let task = sample_task();
        let for_claude = build_brief(&task, " M src/x.ts\n", " 1 file changed\n");
        let for_codex = build_brief(&task, " M src/x.ts\n", " 1 file changed\n");
        assert_eq!(for_claude, for_codex);
        assert!(for_claude.contains("Add a Save button"));
        assert!(for_claude.contains("npm test"));
        assert!(for_claude.contains("sushi-report"));
    }

    #[test]
    fn brief_includes_previous_attempt_failure_truncated() {
        let mut task = sample_task();
        let long_detail = "x".repeat(2000);
        task.attempts.push(Attempt {
            n: 1,
            stage: Stage::Implement,
            route_id: "claude-sonnet".into(),
            harness: Harness::Claude,
            model: "sonnet".into(),
            reason: "tier default".into(),
            session_id: None,
            pgid: None,
            resumed: false,
            started_at: 1,
            ended_at: Some(2),
            status: AttemptStatus::Failed,
            summary: None,
            changed_files: vec![],
            verify: vec![],
            gate_blocks: 0,
            review: None,
            failure: Some(Failure {
                kind: FailureKind::Verify,
                detail: long_detail,
                signature: "verify:xxxx".into(),
            }),
            usage: None,
            cost_usd: None,
        });
        let brief = build_brief(&task, "", "");
        assert!(brief.contains("Previous attempts"));
        assert!(brief.contains("Attempt 1 (claude-sonnet): failed"));
        // truncated to MAX_FAILURE_DETAIL chars plus the ellipsis marker
        let failure_line = brief.lines().find(|l| l.contains("failure:")).unwrap();
        assert!(failure_line.len() < 2000);
    }

    #[test]
    fn resume_delta_contains_only_failure_and_report_format() {
        let failure = Failure {
            kind: FailureKind::Verify,
            detail: "npm test exited 1".into(),
            signature: "verify:npm test exited 1".into(),
        };
        let delta = build_resume_delta(&failure);
        assert!(delta.contains("npm test exited 1"));
        assert!(delta.contains("sushi-report"));
        assert!(!delta.contains("## Task"));
        assert!(!delta.contains("## Acceptance criteria"));
    }

    #[test]
    fn parse_report_reads_last_fence_and_ignores_earlier_ones() {
        let text = "blah\n```sushi-report\n{\"outcome\":\"partial\",\"summary\":\"old\"}\n```\nmore text\n```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"final answer\",\"decisions\":[\"chose X\"],\"question\":\"\"}\n```\n";
        let report = parse_report(text).unwrap();
        assert_eq!(report.outcome, Outcome::Complete);
        assert_eq!(report.summary, "final answer");
        assert_eq!(report.decisions, vec!["chose X".to_string()]);
    }

    #[test]
    fn parse_report_returns_none_when_malformed_or_missing() {
        assert!(parse_report("no fence here").is_none());
        assert!(parse_report("```sushi-report\nnot json\n```").is_none());
        assert!(parse_report("```sushi-report\n{\"outcome\":\"unknown\"}\n```").is_none());
    }

    #[test]
    fn parse_review_reads_verdict_and_findings() {
        let text = "```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"missing test\"]}\n```";
        let review = parse_review(text).unwrap();
        assert_eq!(review.verdict, Verdict::Fail);
        assert_eq!(review.findings, vec!["missing test".to_string()]);
    }
}
