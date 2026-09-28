#[test]
fn variant_route_line_names_the_override() {
    assert_eq!(
        variant_route_line("tier hard", "claude-sonnet"),
        "Variant: tier hard -> route claude-sonnet (override)"
    );
}

#[test]
fn jev_tier_line_formats_choice_probability_and_route() {
    assert_eq!(
        jev_tier_line("mechanical", 0.82, "codex"),
        "Jev: tier mechanical (p 0.82) -> route codex"
    );
}

#[test]
fn jev_tier_fallback_line_formats_reason_and_route() {
    assert_eq!(
        jev_tier_fallback_line("no classifier key", "claude-sonnet"),
        "Jev: tier unavailable (no classifier key) -> fallback standard, route claude-sonnet"
    );
}

#[test]
fn jev_answerable_line_names_the_outcome() {
    assert_eq!(
        jev_answerable_line(0.9, true),
        "Jev: answerable from repo (p 0.90) -> agent sent back"
    );
    assert_eq!(
        jev_answerable_line(0.4, false),
        "Jev: answerable from repo (p 0.40) -> asked owner"
    );
}

#[test]
fn jev_stop_gate_line_names_the_outcome() {
    assert_eq!(
        jev_stop_gate_line(true, 0.9, 0.1),
        "Jev: premature finish (p 0.90) -> sent back"
    );
    assert_eq!(
        jev_stop_gate_line(false, 0.9, 0.85),
        "Jev: verification looks fine (p 0.85) -> allowed"
    );
}

#[test]
fn jev_plan_preflight_line_reports_all_three_scores() {
    assert_eq!(
        jev_plan_preflight_line(0.9, 0.8, 0.3),
        "Jev: goal 0.90, criteria 0.80, verification 0.30"
    );
}

#[test]
fn orchestrator_answer_line_includes_question_answer_and_reason() {
    assert_eq!(
        orchestrator_answer_line("Which theme?", "light", "README says so"),
        "Orchestrator: Which theme? -> light (README says so)"
    );
}

#[test]
fn orchestrator_escalate_line_includes_the_reason() {
    assert_eq!(
        orchestrator_escalate_line("no default anywhere"),
        "Orchestrator: escalated (no default anywhere)"
    );
}
