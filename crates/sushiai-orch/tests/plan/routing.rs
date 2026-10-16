use super::*;

#[test]
fn plan_route_small_request_uses_standard_with_size_facts() {
    let request = "Fix src/button.rs";
    assert_plan_route(
        request,
        1200,
        serde_json::json!({}),
        "claude-sonnet",
        &format!(
            "small request: {} chars <= 1200, 1 paths <= 3, no list",
            request.chars().count()
        ),
    );
}

#[test]
fn plan_route_long_request_uses_planner() {
    assert_plan_route(
        &"x".repeat(1201),
        1200,
        serde_json::json!({}),
        "claude-opus",
        "not small: 1201 chars > 1200",
    );
}

#[test]
fn plan_route_numbered_and_bulleted_lists_use_planner() {
    for request in ["1. Fix button\n2. Add test", "- Fix button\n+ Add test"] {
        assert_plan_route(
            request,
            1200,
            serde_json::json!({}),
            "claude-opus",
            "not small: 2 list items >= 2",
        );
    }
}

#[test]
fn plan_route_variant_override_wins_for_small_request() {
    assert_plan_route(
        "Fix button",
        1200,
        serde_json::json!({"plannerRoute": "claude-opus"}),
        "claude-opus",
        "variant.plannerRoute",
    );
}

#[test]
fn plan_route_zero_turns_off_the_entire_small_request_check() {
    assert_plan_route(
        "Fix button",
        0,
        serde_json::json!({}),
        "claude-opus",
        "small-request check off: smallPlanMaxChars = 0",
    );
}
