use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;

#[test]
fn a_variant_written_with_retired_flags_still_loads_and_drops_them() {
    let old: Variant = serde_json::from_value(serde_json::json!({
        "retryMode": "resume", "leanOutput": true, "reviewBlind": true, "advisor": true
    }))
    .unwrap();
    assert!(old.advisor);
    let json = serde_json::to_string(&old).unwrap();
    for key in Variant::RETIRED_KEYS {
        assert!(!json.contains(key), "{json}");
    }
}

#[test]
fn a_variant_without_route_overrides_serializes_as_before() {
    assert_eq!(
        serde_json::to_string(&Variant::default()).unwrap(),
        r#"{"stallTimeoutSecs":0,"reviewEvidence":false,"advisor":false,"loopDetect":false,"land":false}"#
    );
}

#[test]
fn route_overrides_serialize_in_one_key_order_and_round_trip() {
    let mut a = Variant::default();
    a.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
    a.tier_routes.insert(Tier::Mechanical, "codex".into());
    a.planner_route = Some("claude-sonnet".into());
    let mut b = Variant {
        planner_route: Some("claude-sonnet".into()),
        ..Variant::default()
    };
    b.tier_routes.insert(Tier::Mechanical, "codex".into());
    b.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(json, serde_json::to_string(&b).unwrap());
    assert!(
        json.ends_with(r#""plannerRoute":"claude-sonnet","tierRoutes":{"mechanical":"codex","hard":"claude-sonnet"},"land":false}"#),
        "{json}"
    );
    let back: Variant = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a);
}

#[test]
fn grounded_checks_flag_is_retired() {
    assert!(Variant::RETIRED_KEYS.contains(&"groundedChecks"));
    assert!(!Variant::OPTIONAL_KEYS.contains(&"groundedChecks"));
    assert!(!serde_json::to_string(&Variant::default())
        .unwrap()
        .contains("groundedChecks"));
    // An old task.json that names the flag still loads.
    let mut json = serde_json::to_value(Variant::default()).unwrap();
    json["groundedChecks"] = true.into();
    assert!(serde_json::from_value::<Variant>(json).is_ok());
}

#[test]
fn held_out_defaults_on_and_only_explicit_off_is_written() {
    assert!(Variant::default().held_out);
    assert!(serde_json::from_str::<Variant>("{}").unwrap().held_out);
    assert!(Settings::default().experiments.held_out);
    assert!(Variant::OPTIONAL_KEYS.contains(&"heldOut"));
    assert!(serde_json::to_value(Variant::default())
        .unwrap()
        .get("heldOut")
        .is_none());
    let disabled: Variant = serde_json::from_str(r#"{"heldOut":false}"#).unwrap();
    assert!(!disabled.held_out);
    assert_eq!(serde_json::to_value(disabled).unwrap()["heldOut"], false);
}

#[test]
fn variant_review_and_brief_check_routes_allow_empty_and_auto_only() {
    let s = Settings::default();
    let mut v = Variant {
        review_route: Some(String::new()),
        brief_check_route: Some("auto".to_string()),
        ..Variant::default()
    };
    assert!(v.check_routes(&s.routes).is_ok());
    v.review_route = Some("nope".to_string());
    assert!(v.check_routes(&s.routes).is_err());
}
