/// Owner-visible `task.decisions` lines for an orchestrator outcome, kept as pure
/// formatting helpers so they're unit-testable without a harness run.
pub(super) fn orchestrator_answer_line(question: &str, answer: &str, reason: &str) -> String {
    format!("Orchestrator: {question} -> {answer} ({reason})")
}

/// The decision line for a question the answer policy answered; `by` is
/// `policy` (a rule) or `judge`.
pub(super) fn policy_answer_line(question: &str, answer: &str, by: &str, evidence: &str) -> String {
    format!("Policy: {question} -> {answer} ({by}: {evidence})")
}

/// The start of an overseer decision line made like an earlier answer.
pub(super) const LIKE_PREFIX: &str = "Overseer: answered like ";

/// The decision line for an overseer answer given like an earlier one: `like`
/// names the precedent or repo note it followed.
pub(super) fn overseer_like_line(like: &str, question: &str, answer: &str) -> String {
    format!("{LIKE_PREFIX}{like} -- {question} -> {answer}")
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
