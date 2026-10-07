#[test]
fn resolve_variant_lays_known_flags_over_the_defaults() {
    let defaults = Variant {
        stall_timeout_secs: 900,
        ..Variant::default()
    };
    let v = resolve_variant(&defaults, Some(&json!({"advisor": true}))).unwrap();
    assert!(v.advisor);
    assert_eq!(v.stall_timeout_secs, 900);
    assert_eq!(resolve_variant(&defaults, None).unwrap(), defaults);
    assert!(resolve_variant(&defaults, Some(&json!({"nope": 1}))).is_err());
    assert!(resolve_variant(&defaults, Some(&json!({"advisor": "sideways"}))).is_err());
    assert!(resolve_variant(&defaults, Some(&json!("fresh"))).is_err());
    assert!(resolve_variant(&defaults, Some(&json!({"stallTimeoutSecs": u64::MAX}))).is_err());
}

#[test]
fn resolve_variant_rejects_a_retired_flag_with_a_clear_message() {
    for key in Variant::RETIRED_KEYS {
        let err = resolve_variant(&Variant::default(), Some(&json!({key: true}))).unwrap_err();
        assert!(err.contains(key) && err.contains("retired"), "{err}");
    }
}

#[test]
fn resolve_variant_takes_route_overrides_the_defaults_leave_out() {
    let v = resolve_variant(
        &Variant::default(),
        Some(&json!({"plannerRoute": "claude-sonnet", "tierRoutes": {"hard": "claude-sonnet"}})),
    )
    .unwrap();
    assert_eq!(v.planner_route.as_deref(), Some("claude-sonnet"));
    assert_eq!(v.tier_routes.get(&Tier::Hard).unwrap(), "claude-sonnet");
    let bad_tier = json!({"tierRoutes": {"extreme": "claude-sonnet"}});
    assert!(resolve_variant(&Variant::default(), Some(&bad_tier)).is_err());
}

#[test]
fn a_task_json_carrying_retired_flags_still_loads() {
    let mut task = serde_json::to_value(task_with_status(TaskStatus::Done)).unwrap();
    task["variant"] = json!({
        "retryMode": "fresh", "plannerTier": true, "contract": true,
        "reviewOtherFamily": true, "deferHeavyChecks": true,
        "leanOutput": true, "reviewBlind": true, "advisor": true
    });
    let loaded: Task = serde_json::from_value(task).unwrap();
    assert!(loaded.variant().advisor);
}
