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

fn tier_answer(choice: &str, p: f64) -> classify::Answers {
    let mut answers = classify::Answers::new();
    answers.insert(
        "tier".to_string(),
        classify::Answer {
            choice: Some(choice.to_string()),
            probabilities: Some(HashMap::from([(choice.to_string(), p)])),
            ..Default::default()
        },
    );
    answers
}

#[test]
fn pick_tier_falls_back_to_classifier_off_regardless_of_the_result() {
    let result: Result<classify::Answers, classify::ClassifyError> =
        Err(classify::ClassifyError("network is down".to_string()));
    match pick_tier(&result, true) {
        TierPick::Fallback { reason } => assert_eq!(reason, "classifier off"),
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_falls_back_to_no_classifier_key() {
    let result: Result<classify::Answers, classify::ClassifyError> =
        Err(classify::ClassifyError(classify::NO_KEY_REASON.to_string()));
    match pick_tier(&result, false) {
        TierPick::Fallback { reason } => assert_eq!(reason, "no classifier key"),
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_falls_back_to_classifier_call_failed_and_drops_the_raw_error() {
    let result: Result<classify::Answers, classify::ClassifyError> = Err(classify::ClassifyError(
        "https://openrouter.ai returned 500: secret leak".to_string(),
    ));
    match pick_tier(&result, false) {
        TierPick::Fallback { reason } => {
            assert_eq!(reason, "classifier call failed");
            assert!(!reason.contains("openrouter"));
            assert!(!reason.contains("secret"));
        }
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_falls_back_to_no_tier_answer_when_choice_is_missing() {
    let mut answers = classify::Answers::new();
    answers.insert("tier".to_string(), classify::Answer::default());
    let result: Result<classify::Answers, classify::ClassifyError> = Ok(answers);
    match pick_tier(&result, false) {
        TierPick::Fallback { reason } => assert_eq!(reason, "no tier answer"),
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_falls_back_to_no_tier_answer_when_the_tier_question_is_absent() {
    let result: Result<classify::Answers, classify::ClassifyError> = Ok(classify::Answers::new());
    match pick_tier(&result, false) {
        TierPick::Fallback { reason } => assert_eq!(reason, "no tier answer"),
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_falls_back_when_unsure() {
    let result: Result<classify::Answers, classify::ClassifyError> = Ok(tier_answer("hard", 0.45));
    match pick_tier(&result, false) {
        TierPick::Fallback { reason } => assert_eq!(reason, "unsure: hard p 0.45"),
        TierPick::Classified { .. } => panic!("expected a fallback"),
    }
}

#[test]
fn pick_tier_classifies_a_confident_answer_and_is_not_a_fallback() {
    let result: Result<classify::Answers, classify::ClassifyError> = Ok(tier_answer("hard", 0.82));
    match pick_tier(&result, false) {
        TierPick::Classified { tier, choice, p } => {
            assert_eq!(tier, Tier::Hard);
            assert_eq!(choice, "hard");
            assert_eq!(p, 0.82);
        }
        TierPick::Fallback { reason } => panic!("expected a classified pick, got {reason}"),
    }
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
