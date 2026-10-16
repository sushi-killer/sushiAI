use crate::engine::attempts::context::discarded_block;
use crate::prompts::text::clip_middle;
use crate::prompts::{brief_text, fill};
use crate::task::attempt::{Attempt, AttemptStatus, Stage};
use crate::task::checks::types::{Baseline, Check};
use crate::task::state::Task;

const MAX_FAILURE_DETAIL: usize = 1500;

/// The latest failure is the one the next attempt has to fix, so it gets the whole
/// verify tail rather than the attempt list's short excerpt.
pub(super) const MAX_LATEST_FAILURE_DETAIL: usize = 4500;

fn attempt_outcome_label(attempt: &Attempt) -> &'static str {
    match attempt.status {
        AttemptStatus::Passed => "passed",
        AttemptStatus::Failed => "failed",
        AttemptStatus::Interrupted => "interrupted",
        AttemptStatus::Blocked => "blocked",
        AttemptStatus::Running => "running",
    }
}

/// `## Report format` of an implement brief, which [`with_block_before_report`]
/// looks for.
pub(super) fn report_format_block() -> String {
    format!("{}\n", brief_text("report_format"))
}

pub(super) fn append_check_plan(out: &mut String, task: &Task) {
    out.push_str("## Acceptance criteria\n\n");
    if task.criteria.is_empty() {
        out.push_str("(none specified)\n");
    } else {
        for (i, c) in task.criteria.iter().enumerate() {
            out.push_str(&format!("- [{i}] "));
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

    if let Some(check) = &task.brief_check.check {
        out.push_str(brief_text("targeted_check"));
        out.push_str("\n\n");
        out.push_str("- `");
        out.push_str(check);
        out.push_str("`\n\n");
    }

    let gated: Vec<&Check> = task
        .checks
        .iter()
        .filter(|c| c.baseline == Some(Baseline::Fail))
        .collect();
    if !gated.is_empty() {
        out.push_str(brief_text("checks"));
        out.push_str("\n\n");
        for c in gated {
            out.push_str(&format!("- criterion [{}]: `{}`\n", c.criterion, c.run));
        }
        out.push('\n');
    }
}

/// Full brief for an implement attempt.
pub fn build_brief(task: &Task, git_status_short: &str, git_diff_stat: &str) -> String {
    let mut out = String::new();

    out.push_str("## Task\n\n");
    out.push_str(&task.title);
    out.push_str("\n\n");
    out.push_str(&task.goal);
    out.push_str("\n\n");
    if !task.addenda.is_empty() {
        out.push_str("## Addendum from the owner\n\n");
        for text in &task.addenda {
            out.push_str(&format!("- {}\n", text.replace('\n', "\n  ")));
        }
        out.push('\n');
    }

    append_check_plan(&mut out, task);
    append_plan_context(&mut out, task);

    out.push_str(brief_text("rules"));
    out.push_str("\n\n");

    append_owner_context(&mut out, task);

    append_previous_attempts(&mut out, task);

    out.push_str("## Current state\n\n");
    out.push_str("`git status --short`:\n```\n");
    out.push_str(git_status_short.trim_end());
    out.push_str("\n```\n\n");
    out.push_str("`git diff --stat` (against base):\n```\n");
    out.push_str(git_diff_stat.trim_end());
    out.push_str("\n```\n\n");

    out.push_str(&report_format_block());
    let last_failed_review = task
        .attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .and_then(|a| a.failure.as_ref())
        .is_some_and(|f| f.kind == crate::task::attempt::FailureKind::Review);
    if last_failed_review {
        out.push('\n');
        out.push_str(brief_text("declines"));
        out.push('\n');
    }

    out
}

fn append_plan_context(out: &mut String, task: &Task) {
    if let Some(files) = task
        .planner_tests
        .protected
        .as_ref()
        .filter(|m| !m.is_empty())
    {
        out.push_str(brief_text("protected"));
        out.push_str("\n\n");
        for path in files.keys() {
            out.push_str(&format!("- `{path}`\n"));
        }
        out.push('\n');
    }
    if !task.planner_tests.discarded.is_empty() {
        out.push_str(&discarded_block(&task.planner_tests.discarded));
    }
    out.push_str(brief_text("impossible"));
    out.push_str("\n\n");

    if !task.checklist.is_empty() {
        out.push_str(brief_text("checklist"));
        out.push_str("\n\n");
        for item in &task.checklist {
            let mark = if item.passes { "x" } else { " " };
            if item.check.trim().is_empty() {
                out.push_str(&format!("- [{mark}] {}\n", item.text));
            } else {
                out.push_str(&format!(
                    "- [{mark}] {} -- check: `{}`\n",
                    item.text, item.check
                ));
            }
        }
        out.push('\n');
    }

    if !task.plan_gate.findings.is_empty() {
        out.push_str(brief_text("plan_notes"));
        out.push_str("\n\n");
        for f in &task.plan_gate.findings {
            out.push_str(&format!(
                "- {} ({}): {}\n",
                f.severity.label(),
                f.kind,
                f.text
            ));
        }
        out.push('\n');
    }

    append_final_verify(out, task);
}

fn append_owner_context(out: &mut String, task: &Task) {
    if !task.decisions.is_empty() {
        out.push_str("## Decisions and owner answers\n\n");
        for d in &task.decisions {
            out.push_str("- ");
            out.push_str(d);
            out.push('\n');
        }
        out.push('\n');
    }

    if !task.assumptions.is_empty() {
        out.push_str(brief_text("assumptions"));
        out.push_str("\n\n");
        for a in &task.assumptions {
            match (&a.owner_answer, a.overturned) {
                (Some(owner), true) => {
                    out.push_str(&fill(
                        brief_text("assumption_overturned"),
                        &[
                            ("question", &a.question),
                            ("owner", owner),
                            ("answer", &a.answer),
                        ],
                    ));
                    out.push('\n');
                }
                _ => out.push_str(&format!(
                    "- {} -> {} ({})\n",
                    a.question, a.answer, a.evidence
                )),
            }
        }
        out.push('\n');
    }
}

fn append_previous_attempts(out: &mut String, task: &Task) {
    // The plan attempt (if any) drafted the brief the agent is reading right
    // now, not a previous *implement* try -- it has no place in this list.
    let implement_attempts: Vec<&Attempt> = task
        .attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .collect();
    if !implement_attempts.is_empty() {
        out.push_str("## Previous attempts\n\n");
        let last = implement_attempts.len() - 1;
        for (i, attempt) in implement_attempts.into_iter().enumerate() {
            out.push_str(&format!(
                "- Attempt {} ({}): {}\n",
                attempt.n,
                attempt.route_id,
                attempt_outcome_label(attempt)
            ));
            if let Some(handoff) = attempt.handoff.as_deref().filter(|h| !h.trim().is_empty()) {
                out.push_str("  - handoff: ");
                out.push_str(&clip_middle(handoff, MAX_FAILURE_DETAIL));
                out.push('\n');
            }
            if let Some(failure) = &attempt.failure {
                // The latest failure is what this attempt has to fix: give it
                // more room than the earlier ones.
                let max = if i == last {
                    MAX_LATEST_FAILURE_DETAIL
                } else {
                    MAX_FAILURE_DETAIL
                };
                let detail = clip_middle(&failure.detail, max);
                out.push_str("  - failure: ");
                if failure.kind == crate::task::attempt::FailureKind::Loop {
                    out.push_str("stopped as a loop. ");
                }
                out.push_str(&detail);
                out.push('\n');
            }
        }
        out.push('\n');
    }
}

pub(super) fn append_final_verify(out: &mut String, task: &Task) {
    if !task.final_verify.is_empty() {
        out.push_str(brief_text("final_checks"));
        out.push_str("\n\n");
        for v in &task.final_verify {
            out.push_str("- `");
            out.push_str(v);
            out.push_str("`\n");
        }
        out.push('\n');
    }
}

#[cfg(test)]
#[path = "tests/brief.rs"]
mod tests;
