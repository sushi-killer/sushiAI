/// Owner-visible `task.decisions` lines for a triage outcome, kept as pure
/// formatting helpers so they're unit-testable without a harness run.
pub(super) fn orchestrator_answer_line(question: &str, answer: &str, reason: &str) -> String {
    format!("Orchestrator: {question} -> {answer} ({reason})")
}

/// The decision line for a question the answer policy answered; `by` is
/// `policy` (a rule) or `judge`.
pub(super) fn policy_answer_line(question: &str, answer: &str, by: &str, evidence: &str) -> String {
    format!("Policy: {question} -> {answer} ({by}: {evidence})")
}

pub(super) fn orchestrator_escalate_line(reason: &str) -> String {
    format!("Orchestrator: escalated ({reason})")
}

/// A decision line produced by [`orchestrator_answer_line`] specifically --
/// `false` for an escalation line, an owner/agent line, or anything else.
/// Used to cap triage to one *answer* in a row (P1-2).
pub(super) fn is_orchestrator_answer_decision(line: &str) -> bool {
    line.starts_with("Orchestrator: ")
        && line.contains(" -> ")
        && !line.starts_with(NO_PLANNER_TIER_PREFIX)
}

/// Recorded once per task and stage/tier when a variant's route override
/// replaces the settings' route.
pub(super) fn variant_route_line(what: &str, route_id: &str) -> String {
    format!("Variant: {what} -> route {route_id} (override)")
}

/// Recorded when a task has no planner tier and runs on the standard route.
pub(super) fn no_planner_tier_line(route_id: &str) -> String {
    format!("{NO_PLANNER_TIER_PREFIX} -> standard, route {route_id}")
}

const NO_PLANNER_TIER_PREFIX: &str = "Orchestrator: no planner tier";
