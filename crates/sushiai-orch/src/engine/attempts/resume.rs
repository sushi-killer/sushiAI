use super::brief::append_final_verify;
use crate::engine::attempts::advisor::advisor_block;
use crate::engine::attempts::brief::{
    append_check_plan, report_format_block, MAX_LATEST_FAILURE_DETAIL,
};
use crate::engine::attempts::context::discarded_block;
use crate::prompts::brief_text;
use crate::prompts::text::{clip_middle, tail_chars};
use crate::task::attempt::{Attempt, FailureKind};
use crate::task::state::Task;
use crate::verify::evidence::is_held_out;

/// A resumed implementer already has the task and its earlier work in context.
pub fn build_resume_brief(
    task: &Task,
    previous: &Attempt,
    messages: &str,
    advice: Option<&str>,
) -> String {
    let timed_out = previous
        .failure
        .as_ref()
        .is_some_and(|f| f.kind == FailureKind::TimeBudget);
    let mut out = format!(
        "{}\n\n{}\n\n",
        brief_text(if timed_out {
            "resume_time_budget"
        } else {
            "resume"
        }),
        brief_text("current_plan")
    );
    append_check_plan(&mut out, task);
    append_final_verify(&mut out, task);
    out.push_str(brief_text("previous_attempt_evidence"));
    out.push_str("\n\n");
    if let Some(failure) = &previous.failure {
        out.push_str(&clip_middle(&failure.detail, MAX_LATEST_FAILURE_DETAIL));
        out.push_str("\n\n");
    }
    for result in previous
        .verify
        .iter()
        .filter(|v| v.code != Some(0) && !is_held_out(v))
    {
        if previous
            .failure
            .as_ref()
            .is_some_and(|f| f.detail.contains(&result.command))
        {
            continue;
        }
        out.push_str(&format!(
            "`{}` exited {}.\n{}\n\n",
            result.command,
            result
                .code
                .map_or_else(|| "without an exit code".into(), |code| code.to_string()),
            tail_chars(&result.tail, MAX_LATEST_FAILURE_DETAIL)
        ));
    }
    if let Some(handoff) = &previous.handoff {
        out.push_str("## Previous handoff\n\n");
        out.push_str(&clip_middle(handoff, MAX_LATEST_FAILURE_DETAIL));
        out.push_str("\n\n");
    }
    out.push_str(messages);
    if !task.planner_tests.discarded.is_empty() {
        out.push_str("\n\n");
        out.push_str(&discarded_block(&task.planner_tests.discarded));
    }
    let answers: Vec<_> = task
        .question_history
        .iter()
        .filter(|q| {
            q.answered_at > previous.started_at
                && q.answered_by == crate::task::questions::AnsweredBy::Owner
        })
        .collect();
    if !answers.is_empty() {
        out.push_str("\n\n## Owner answers\n\n");
        for q in answers {
            out.push_str(&format!("- {} -> {}\n", q.question, q.answer));
        }
    }
    if let Some(advice) = advice {
        out.push_str("\n\n");
        out.push_str(&advisor_block(advice));
    }
    out.push_str("\n\n");
    out.push_str(&report_format_block());
    out
}

#[cfg(test)]
#[path = "tests/resume.rs"]
mod tests;
