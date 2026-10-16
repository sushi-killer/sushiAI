use crate::prompts::engine_text;
use crate::task::{PlanFinding, Task};
use serde_json::json;

pub(in crate::engine) fn build_review_prompt(
    task: &Task,
    request: &str,
    subtasks: &[crate::engine::planning::draft::PlanSubtask],
) -> String {
    let parts: Vec<serde_json::Value> = subtasks
        .iter()
        .map(|s| {
            json!({"key": s.key, "title": s.title, "request": s.request,
                   "goal": s.goal, "criteria": s.criteria, "verify": s.verify,
                   "finalVerify": s.final_verify, "checks": s.checks,
                   "heldOutCriterion": s.held_out.as_ref().map(|h| h.criterion), "tests": s.tests,
                   "visualCriteria": s.visual_criteria, "screenshot": s.screenshot,
                   "tier": s.tier, "dependsOn": s.depends_on,
                   "paths": s.paths, "check": s.check_command()})
        })
        .collect();
    format!(
        "{}\n\n{}\n",
        engine_text("plan_gate_review"),
        json!({
            "request": request,
            "goal": task.goal,
            "criteria": task.criteria,
            "checks": task.checks,
            "heldOutCriterion": task.held_out.as_ref().map(|h| h.criterion),
            "tests": task.planner_tests.files,
            "verify": task.verify,
            "finalVerify": task.final_verify,
            "paths": task.paths,
            "subtasks": parts,
        })
    )
}

/// The `## Plan gate findings` block of a replan brief.
pub(in crate::engine) fn findings_block(findings: &[PlanFinding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut out = format!("{}\n\n", engine_text("plan_gate_findings"));
    for f in findings {
        out.push_str(&format!(
            "- {} ({}): {}\n",
            f.severity.label(),
            f.kind,
            f.text
        ));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
