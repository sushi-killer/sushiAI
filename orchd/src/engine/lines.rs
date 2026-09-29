/// Owner-visible `task.decisions` lines for a triage outcome, kept as pure
/// formatting helpers (same convention as the `jev_*` ones above) so
/// they're unit-testable without a harness run.
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
    line.starts_with("Orchestrator: ") && line.contains(" -> ")
}

/// Recorded once per task and stage/tier when a variant's route override
/// replaces the settings' route.
pub(super) fn variant_route_line(what: &str, route_id: &str) -> String {
    format!("Variant: {what} -> route {route_id} (override)")
}

/// Owner-visible `task.decisions` lines for a successful classifier call,
/// kept as pure formatting helpers so they're unit-testable without a
/// network call. Never fed anything but probabilities/choices/route ids --
/// no key, base URL, or raw response body ever reaches these.
pub(super) fn jev_tier_line(choice: &str, p: f64, route_id: &str) -> String {
    format!("Jev: tier {choice} (p {p:.2}) -> route {route_id}")
}

pub(super) fn jev_tier_fallback_line(reason: &str, route_id: &str) -> String {
    format!("Jev: tier unavailable ({reason}) -> fallback standard, route {route_id}")
}

pub(super) fn jev_answerable_line(p: f64, answer_self: bool) -> String {
    let outcome = if answer_self {
        "agent sent back"
    } else {
        "asked owner"
    };
    format!("Jev: answerable from repo (p {p:.2}) -> {outcome}")
}

pub(super) fn jev_stop_gate_line(blocked: bool, claims_done: f64, claims_verified: f64) -> String {
    if blocked {
        format!("Jev: premature finish (p {claims_done:.2}) -> sent back")
    } else {
        format!("Jev: verification looks fine (p {claims_verified:.2}) -> allowed")
    }
}
