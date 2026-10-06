#[test]
fn variant_route_line_names_the_override() {
    assert_eq!(
        variant_route_line("tier hard", "claude-sonnet"),
        "Variant: tier hard -> route claude-sonnet (override)"
    );
}

#[test]
fn no_planner_tier_line_names_the_standard_route() {
    let line = no_planner_tier_line("claude-sonnet");
    assert_eq!(
        line,
        "Orchestrator: no planner tier -> standard, route claude-sonnet"
    );
    assert!(!is_orchestrator_answer_decision(&line));
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
