use crate::policy::settings::ScopedCheck;
use crate::policy::variant::Variant;
use crate::prompts::{brief_text, fill};

/// The drafting-stage brief for a top-level task: the owner's request
/// verbatim, then the fixed planning instructions and report format (spec:
/// "harness-agnostic", so this takes no harness parameter, same as
/// [`build_brief`]). The planner may split the request into subtasks.
pub fn build_plan_brief(request: &str, variant: &Variant, past_work: &str) -> String {
    plan_brief(request, variant, past_work, true)
}

/// The drafting-stage brief for a subtask: the same, without the option to
/// split again (a subtask never has subtasks of its own).
pub fn build_subtask_plan_brief(request: &str, variant: &Variant, past_work: &str) -> String {
    plan_brief(request, variant, past_work, false)
}

/// The `sushi-plan` report format the variant and the split choice ask for;
/// a fresh draft and a correction turn end with the same one.
fn plan_report_format(variant: &Variant, split: bool) -> String {
    let question_fields = if variant.batch_questions {
        brief_text("plan_report_question_fields")
    } else {
        ""
    };
    let subtasks = if split {
        brief_text("plan_report_subtasks")
    } else {
        ""
    };
    let format = fill(
        brief_text("plan_report_format"),
        &[("question_fields", question_fields), ("subtasks", subtasks)],
    );
    format!("{format}\n")
}

fn plan_brief(request: &str, variant: &Variant, past_work: &str, split: bool) -> String {
    let mut extra = String::new();
    let mut paragraph = |key: &str| {
        extra.push_str("\n\n");
        extra.push_str(brief_text(key));
    };
    paragraph("plan_contract");
    paragraph("plan_coverage");
    if variant.batch_questions {
        paragraph("plan_batch_questions");
    }
    paragraph("plan_paths");
    paragraph("plan_search_rule");
    paragraph("plan_final_verify");
    paragraph("plan_checks");
    extra.push(' ');
    extra.push_str(brief_text("plan_tests"));
    extra.push_str("\n\n");
    extra.push_str(brief_text("plan_measurement"));
    if split {
        extra.push_str("\n\n");
        extra.push_str(brief_text("plan_subtasks"));
        extra.push(' ');
        extra.push_str(brief_text("plan_subtask_checks"));
    } else {
        extra.push_str("\n\n");
        extra.push_str(brief_text("plan_subtask_gate"));
    }
    let format = plan_report_format(variant, split);
    let past_work = if past_work.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", past_work.trim_end())
    };
    format!(
        "## Request\n\n{}\n\n{past_work}## Instructions\n\n{}{extra}\n\n{format}",
        request.trim(),
        brief_text("plan_instructions"),
    )
}

/// Sent into the planner's own session (resumed) when the brief check found
/// two requirements of its draft contradicting each other: only the
/// conflict, the instruction to change nothing else, and the report format.
/// The session already holds the request, the repo and the previous plan.
pub fn build_plan_correction_brief(conflict: &str, variant: &Variant, split: bool) -> String {
    format!(
        "{}\n\n{}",
        fill(
            brief_text("plan_correction"),
            &[("conflict", conflict.trim())]
        ),
        plan_report_format(variant, split),
    )
}

/// The brief of a planner run resumed after its first run timed out or
/// errored: the session already holds the request and what it read, so it only
/// continues and returns the plan in the report format.
pub fn build_plan_continue_brief(variant: &Variant, split: bool) -> String {
    format!(
        "{}\n\n{}",
        brief_text("plan_continue"),
        plan_report_format(variant, split),
    )
}

/// The `## Scoped checks` block of a plan brief: the settings' scoped checks
/// that apply to `repo`; empty when none do.
pub fn scoped_checks_block(scoped: &[ScopedCheck], repo: &str) -> String {
    let lines: Vec<String> = scoped
        .iter()
        .filter(|e| !e.paths.is_empty() && e.repo.as_deref().is_none_or(|r| r == repo))
        .map(|e| {
            fill(
                brief_text("scoped_check"),
                &[("command", &e.command), ("paths", &e.paths.join(", "))],
            )
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let block = fill(
        brief_text("scoped_checks"),
        &[("checks", &lines.join("\n"))],
    );
    format!("{block}\n\n")
}

/// Sent instead of [`build_plan_brief`] on the one retry after an
/// unparseable draft -- a fresh read-only session (an unparseable reply has
/// no plan to correct; only a brief-check contradiction resumes the planner,
/// see [`build_plan_correction_brief`]), so it still needs the full request,
/// just with an explicit reminder in front of it.
pub fn build_plan_retry_brief(request: &str, variant: &Variant, past_work: &str) -> String {
    plan_retry(&build_plan_brief(request, variant, past_work))
}

pub fn build_subtask_plan_retry_brief(request: &str, variant: &Variant, past_work: &str) -> String {
    plan_retry(&build_subtask_plan_brief(request, variant, past_work))
}

fn plan_retry(brief: &str) -> String {
    format!("{}\n\n{brief}", brief_text("plan_retry"))
}

#[cfg(test)]
#[path = "tests/prompt.rs"]
mod tests;
