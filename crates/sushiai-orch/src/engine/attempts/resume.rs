use super::brief::append_final_verify;
use crate::clip::tail_chars;
use crate::engine::attempts::advisor::advisor_block;
use crate::engine::attempts::brief::{
    append_check_plan, report_format_block, MAX_LATEST_FAILURE_DETAIL,
};
use crate::engine::attempts::context::discarded_block;
use crate::prompts::brief_text;
use crate::prompts::text::clip_middle;
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

/// The current brief and the full input retained if a resumed session is rejected.
pub(in crate::engine) struct RunBrief {
    pub(in crate::engine) current: String,
    pub(in crate::engine) full: String,
    pub(in crate::engine) session: Option<String>,
}

impl RunBrief {
    pub(in crate::engine) fn build(
        task: &mut crate::task::state::Task,
        selected: &crate::engine::lifecycle::selection::Selection,
        full: crate::engine::lifecycle::implementation_brief::FullBrief,
        has_second: bool,
    ) -> Self {
        let previous =
            crate::engine::lifecycle::resume::resume_attempt(task, &selected.route, has_second);
        let session = previous.and_then(|attempt| attempt.session_id.clone());
        let mut current = match previous {
            Some(attempt) => {
                build_resume_brief(task, attempt, &full.messages, full.advice.as_deref())
            }
            None => full.text.clone(),
        };
        let reason = if previous
            .and_then(|attempt| attempt.failure.as_ref())
            .is_some_and(|failure| failure.kind == crate::task::attempt::FailureKind::TimeBudget)
        {
            "its time budget"
        } else {
            "a failed check"
        };
        let mut full_text = full.text;
        if !selected.carry_conflicts.is_empty() {
            let note = crate::engine::attempts::context::carry_conflict_block(
                &selected.carry_conflicts,
                &task.id,
            );
            current = crate::engine::attempts::context::with_block_before_report(&current, &note);
            full_text =
                crate::engine::attempts::context::with_block_before_report(&full_text, &note);
        }
        if task.pr_check_repair_due() {
            if let Some(repair) = &task.queue.pr_check_repair {
                let block = crate::engine::attempts::context::pr_check_repair_block(repair);
                current =
                    crate::engine::attempts::context::with_block_before_report(&current, &block);
                full_text =
                    crate::engine::attempts::context::with_block_before_report(&full_text, &block);
            }
        }
        if let Some(session) = &session {
            let attempt_n = crate::task::attempts::next_number(task);
            task.record_decision(format!("Orchestrator: attempt {attempt_n} resumes session {session} on route {} after {reason}", selected.route.id));
        }
        Self {
            current,
            full: full_text,
            session,
        }
    }
}
