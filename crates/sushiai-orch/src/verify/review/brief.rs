use crate::clip::tail_chars;
use crate::prompts::text::untrusted_block;
use crate::prompts::{brief_text, fill};
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::commands::held_out_label;
use crate::task::checks::types::Baseline;
use crate::task::review::{Decline, DeclineReason, FindingStatus};
use crate::task::state::Task;
use crate::verify::evidence::is_held_out;
use crate::verify::review::routing::{review_lens_block, Lenses};

/// Tells the reviewer about the same conflict.
pub fn review_conflict_block(conflict: &str) -> String {
    fill(brief_text("review_conflict"), &[("conflict", conflict)])
}

/// The review brief's section for the declines of the attempt under review.
/// The reviewer rules on each one by its id in `declineRulings`.
fn declines_block(declines: &[Decline]) -> String {
    if declines.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = declines
        .iter()
        .map(|d| {
            let reason = match d.reason {
                DeclineReason::Wrong => "wrong",
                DeclineReason::AlreadySatisfied => "already_satisfied",
                DeclineReason::ChangesBehaviour => "changes_behaviour",
            };
            format!("{} ({reason}): {}", d.id, d.evidence.join("; "))
        })
        .collect();
    let mut out = format!("\n{}\n\n", brief_text("review_declines"));
    out.push_str(&untrusted_block(
        "The implementer's declines",
        &lines.join("\n"),
    ));
    out.push('\n');
    out
}

/// Implementer output and recorded evidence shared by full and retry reviews.
/// Screenshot paths are relative to the worktree and name their source attempt.
pub struct ReviewEvidence<'a> {
    pub implementer_note: &'a str,
    pub agent_decisions: &'a [String],
    pub verify_results: &'a [VerifyOutcome],
    pub screenshots: &'a [(String, Option<u32>)],
    pub evidence_from: Option<u32>,
    pub declines: &'a [Decline],
}

pub fn build_review_brief(
    task: &Task,
    evidence: &ReviewEvidence<'_>,
    diff: &str,
    lenses: &Lenses,
) -> String {
    review_brief(task, evidence, ReviewBody::Full { diff, lenses })
}

/// What a retry review is asked to look at instead of the whole change.
pub struct NarrowReview<'a> {
    /// Criteria (index, text) that review did not rule met: ruled on now.
    pub open_criteria: &'a [(usize, String)],
    /// Criteria (index, text) that review ruled met: not checked again.
    pub settled_criteria: &'a [(usize, String)],
    /// The task's own changes since the reviewed tree.
    pub delta: &'a str,
}

enum ReviewBody<'a> {
    Full { diff: &'a str, lenses: &'a Lenses },
    Narrow(&'a NarrowReview<'a>),
}

/// The brief of a review after a failed review: the open findings, the
/// criteria still to rule on and the task's own changes since the reviewed
/// tree, instead of every criterion and the full diff. Everything else is the
/// full review's brief.
pub fn build_narrow_review_brief(
    task: &Task,
    evidence: &ReviewEvidence<'_>,
    narrow: &NarrowReview<'_>,
) -> String {
    review_brief(task, evidence, ReviewBody::Narrow(narrow))
}

fn review_brief(task: &Task, evidence: &ReviewEvidence<'_>, body: ReviewBody<'_>) -> String {
    let ReviewEvidence {
        implementer_note,
        agent_decisions,
        verify_results,
        declines,
        ..
    } = *evidence;
    let mut out = String::new();
    out.push_str("## Review\n\n");
    out.push_str(&task.goal);
    append_scope(&mut out, task, &body);
    out.push_str("\n## Implementer\n\n");
    out.push_str(implementer_note);
    if !agent_decisions.is_empty() {
        out.push_str("\n\n");
        out.push_str(&untrusted_block(
            "The implementer's decisions in this attempt",
            &agent_decisions.join("\n"),
        ));
    }
    out.push('\n');
    out.push_str(brief_text("review_deviation"));
    out.push_str(&grounded_checks_block(task, verify_results));
    if let Some(conflict) = &task.brief_check.conflict {
        out.push_str("\n\n");
        out.push_str(&review_conflict_block(conflict));
    }
    out.push_str("\n\n");
    out.push_str(brief_text("review_process"));
    out.push_str("\n\n");
    out.push_str(brief_text("review_verdict_rule"));
    out.push('\n');
    out.push_str(brief_text("review_scope_rule"));
    out.push('\n');
    if let ReviewBody::Full { lenses, .. } = &body {
        out.push_str(&review_lens_block(lenses));
    }
    append_evidence(&mut out, evidence, task.variant().review_evidence);
    if !task.findings.is_empty() {
        let registry: Vec<String> = task
            .findings
            .iter()
            .map(|f| {
                let status = match f.status {
                    FindingStatus::Open => "open",
                    FindingStatus::Resolved => "resolved",
                };
                format!("{} ({status}): {}", f.id, f.text)
            })
            .collect();
        out.push_str("\n## Previous review\n\n");
        out.push_str(&untrusted_block(
            "The findings earlier reviews reported",
            &registry.join("\n"),
        ));
        out.push('\n');
        out.push_str(brief_text("review_previous"));
        out.push('\n');
    }
    out.push_str(&declines_block(declines));
    match &body {
        ReviewBody::Full { diff, .. } => {
            out.push_str("\n## Diff\n\n```diff\n");
            out.push_str(diff);
        }
        ReviewBody::Narrow(narrow) => {
            out.push('\n');
            out.push_str(brief_text("review_narrow_changes"));
            out.push_str("\n\n```diff\n");
            out.push_str(narrow.delta);
        }
    }
    out.push_str("\n```\n\n## Report format\n\nReply with:\n\n```sushi-review\n");
    out.push_str(brief_text("review_report_example"));
    out.push_str("\n```\n");
    out.push('\n');
    out.push_str(review_contract());
    out.push('\n');
    out
}

/// Appended to the brief of a review re-run after a reply with no parseable
/// verdict: what went wrong and the exact format to answer in.
pub fn review_retry_note() -> String {
    let note = fill(
        brief_text("review_retry_note"),
        &[("example", brief_text("review_report_example"))],
    );
    format!("{note}\n")
}

/// The review's account of the grounded checks: which ones prove nothing,
/// and the held-out result by criterion index and text only.
fn grounded_checks_block(task: &Task, verify_results: &[VerifyOutcome]) -> String {
    let mut out = String::new();
    let text = |i: usize| task.criteria.get(i).map(String::as_str).unwrap_or("");
    for c in task
        .checks
        .iter()
        .filter(|c| c.baseline == Some(Baseline::Pass))
    {
        out.push_str(&fill(
            brief_text("grounded_check_ungrounded"),
            &[
                ("criterion", &c.criterion.to_string()),
                ("text", text(c.criterion)),
                ("run", &c.run),
            ],
        ));
        out.push('\n');
    }
    if let Some(h) = &task.held_out {
        let label = held_out_label(h.criterion);
        let line = match verify_results.iter().find(|v| v.command == label) {
            Some(v) if v.code == Some(0) => "passed".to_string(),
            Some(_) => "failed".to_string(),
            None if h.baseline == Some(Baseline::Fail) => "not run".to_string(),
            None => "not grounded, not run".to_string(),
        };
        out.push_str(&fill(
            brief_text("grounded_check_held_out"),
            &[
                ("criterion", &h.criterion.to_string()),
                ("text", text(h.criterion)),
                ("result", &line),
            ],
        ));
        out.push('\n');
    }
    if out.is_empty() {
        return out;
    }
    format!("\n## Grounded checks\n\n{out}")
}

/// Asks the reviewer for a ruling on every criterion.
pub fn review_contract() -> &'static str {
    brief_text("review_contract")
}

fn append_scope(out: &mut String, task: &Task, body: &ReviewBody<'_>) {
    match &body {
        ReviewBody::Full { .. } => {
            out.push_str("\n\n## Acceptance criteria\n\nEach criterion has a number; rule on it by that number.\n\n");
            for (i, c) in task.criteria.iter().enumerate() {
                out.push_str(&format!("{i}. {c}\n"));
            }
        }
        ReviewBody::Narrow(narrow) => {
            out.push_str("\n\n## Criteria to rule on now\n\n");
            if narrow.open_criteria.is_empty() {
                out.push_str(brief_text("review_no_open_criteria"));
                out.push('\n');
            }
            for (i, c) in narrow.open_criteria {
                out.push_str(&format!("{i}. {c}\n"));
            }
            if !narrow.settled_criteria.is_empty() {
                out.push('\n');
                out.push_str(brief_text("review_settled"));
                out.push_str("\n\n");
                for (i, c) in narrow.settled_criteria {
                    out.push_str(&format!("{i}. {c}\n"));
                }
            }
        }
    }
}

fn append_evidence(out: &mut String, evidence: &ReviewEvidence<'_>, detailed: bool) {
    let ReviewEvidence {
        verify_results,
        screenshots,
        evidence_from,
        ..
    } = *evidence;
    out.push_str("\n## Verify results\n\n");
    for v in verify_results.iter().filter(|v| !is_held_out(v)) {
        out.push_str(&format!(
            "- `{}` -> exit {:?}\n```\n{}\n```\n",
            v.command,
            v.code,
            tail_chars(v.tail.trim(), if detailed { 3000 } else { 800 })
        ));
    }
    if !screenshots.is_empty() {
        let carry = evidence_from
            .map(|n| {
                let note = fill(
                    brief_text("review_screenshots_carry"),
                    &[("attempt", &n.to_string())],
                );
                format!(" {note}")
            })
            .unwrap_or_default();
        out.push('\n');
        out.push_str(&fill(
            brief_text("review_screenshots"),
            &[("carry", &carry)],
        ));
        out.push_str("\n\n");
        for (shown, attempt) in screenshots {
            match attempt {
                Some(n) => out.push_str(&format!("- `{shown}` (attempt {n})\n")),
                None => out.push_str(&format!("- `{shown}`\n")),
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/brief.rs"]
mod tests;
