use crate::policy::evolution::EvolutionSettings;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;

#[test]
fn default_settings_have_no_haiku_route_and_check_on_the_cheapest_available_route() {
    let settings = Settings::default();
    assert!(
        !settings
            .routes
            .iter()
            .any(|r| r.id.contains("haiku")
                || r.model.as_deref().is_some_and(|m| m.contains("haiku")))
    );
    assert_eq!(settings.brief_check_route, "auto");
}

#[test]
fn plan_review_defaults_to_codex_and_an_older_settings_file_gets_it() {
    assert_eq!(Settings::default().plan_review, "codex");
    let mut v = serde_json::to_value(Settings::default()).unwrap();
    v.as_object_mut().unwrap().remove("planReview");
    let loaded: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(loaded.plan_review, "codex");
    let mut v = serde_json::to_value(Settings::default()).unwrap();
    v["planReview"] = serde_json::json!("");
    let off: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(off.plan_review, "");
}

#[test]
fn settings_without_evolution_load_with_evolution_defaults() {
    let mut v = serde_json::to_value(Settings::default()).unwrap();
    v.as_object_mut().unwrap().remove("evolution");
    let s: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(s.evolution, EvolutionSettings::default());
    assert_eq!(s.evolution.min_tasks, 3);
    assert_eq!(s.evolution.min_wasted_calls, 20);
    assert_eq!(s.evolution.min_wasted_usd, 1.0);
    assert_eq!(s.evolution.max_proposals, 3);
    assert_eq!(s.evolution.proposer_route, "");
    assert_eq!(s.evolution.revert_after_tasks, 10);
    let partial: EvolutionSettings = serde_json::from_str(r#"{"minTasks":5}"#).unwrap();
    assert_eq!(partial.min_tasks, 5);
    assert_eq!(partial.max_proposals, 3);
}

#[test]
fn settings_default_round_trips_through_json() {
    let s = Settings::default();
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["review"], "auto");
    assert_eq!(v["tiers"]["mechanical"], "codex");
    assert_eq!(v["tiers"]["standard"], "claude-sonnet");
    assert_eq!(v["tiers"]["hard"], "claude-opus");
    assert_eq!(v["maxAttempts"], 4);
    assert_eq!(v["parallel"], 2);
    assert_eq!(v["planner"], "claude-opus");
    assert_eq!(v["orchestrator"], "");
    let back: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(back.max_attempts, 4);
    assert_eq!(back.tiers.get(&Tier::Hard).unwrap(), "claude-opus");
}

#[test]
fn settings_default_without_a_planner_field_defaults_to_claude_opus() {
    // An older settings.json on disk (written before this field existed)
    // must still load, with planning on by default and on the hard
    // tier's route - the strongest model plans best.
    let old_json = serde_json::json!({
        "routes": [], "tiers": {}, "review": "",
        "sandbox": "host", "allowedDomains": [], "protectedPaths": [], "maxAttempts": 4, "parallel": 2,
    });
    let s: Settings = serde_json::from_value(old_json).unwrap();
    assert_eq!(s.planner, "claude-opus");
    assert_eq!(s.orchestrator, "");
    assert!(!s.auto_answer);
}

#[test]
fn plan_timeout_defaults_to_thirty_minutes_and_old_settings_load_it() {
    assert_eq!(Settings::default().plan_timeout_secs, 1800);
    let mut json = serde_json::to_value(Settings::default()).unwrap();
    json.as_object_mut().unwrap().remove("planTimeoutSecs");
    let settings: Settings = serde_json::from_value(json).unwrap();
    assert_eq!(settings.plan_timeout_secs, 1800);
}

#[test]
fn old_settings_default_small_planning_and_repo_mcp_keys() {
    let mut json = serde_json::to_value(Settings::default()).unwrap();
    for key in [
        "smallPlanMaxChars",
        "smallPlanMaxPaths",
        "mcpEnable",
        "mcpDisable",
    ] {
        json.as_object_mut().unwrap().remove(key);
    }
    let settings: Settings = serde_json::from_value(json).unwrap();
    assert_eq!(settings.small_plan_max_chars, 1200);
    assert_eq!(settings.small_plan_max_paths, 3);
    assert!(settings.mcp_enable.is_empty());
    assert!(settings.mcp_disable.is_empty());
    let mut json = serde_json::to_value(settings).unwrap();
    json["smallPlanMaxChars"] = serde_json::json!(0);
    json["smallPlanMaxPaths"] = serde_json::json!(0);
    json["mcpEnable"] = serde_json::json!({"/repo": ["tool"]});
    json["mcpDisable"] = serde_json::json!({"/repo": ["server"]});
    let settings: Settings = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(settings.small_plan_max_chars, 0);
    assert_eq!(settings.small_plan_max_paths, 0);
    assert_eq!(settings.mcp_enable["/repo"], vec!["tool"]);
    assert_eq!(settings.mcp_disable["/repo"], vec!["server"]);
    assert_eq!(serde_json::to_value(settings).unwrap(), json);
}

#[test]
fn settings_written_with_retired_flags_still_load() {
    let retired = serde_json::json!({
        "retryMode": "fresh", "plannerTier": true, "contract": true,
        "reviewOtherFamily": true, "deferHeavyChecks": true,
        "leanOutput": true, "reviewBlind": true, "advisor": true
    });
    let mut settings = serde_json::to_value(Settings::default()).unwrap();
    settings["experiments"] = retired.clone();
    let s: Settings = serde_json::from_value(settings).unwrap();
    assert!(s.experiments.advisor && s.experiments.loop_detect);
}

#[test]
fn old_settings_without_loop_detect_keep_it_on() {
    let parse = |experiments: Option<&str>| -> Settings {
        let mut v = serde_json::to_value(Settings::default()).unwrap();
        let o = v.as_object_mut().unwrap();
        match experiments {
            Some(e) => o.insert("experiments".into(), serde_json::from_str(e).unwrap()),
            None => o.remove("experiments"),
        };
        serde_json::from_value(v).unwrap()
    };
    let s = parse(Some(r#"{"advisor":true}"#));
    assert!(s.experiments.loop_detect && s.experiments.advisor);
    assert!(parse(None).experiments.loop_detect);
    assert!(
        !parse(Some(r#"{"loopDetect":false}"#))
            .experiments
            .loop_detect
    );
}

#[test]
fn route_overrides_replace_the_settings_routes_and_escalation_follows_them() {
    let settings = Settings::default();
    let mut v = Variant::default();
    assert_eq!(
        v.implement_route_id(&settings, Tier::Hard),
        ("claude-opus".to_string(), false)
    );
    assert_eq!(v.plan_route_id(&settings), "claude-opus");
    v.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
    v.planner_route = Some("claude-sonnet".into());
    // A standard-tier task that keeps failing moves up to hard, and
    // takes the overridden hard route.
    assert_eq!(
        v.implement_route_id(&settings, Tier::Standard.up()),
        ("claude-sonnet".to_string(), true)
    );
    assert_eq!(
        v.implement_route_id(&settings, Tier::Mechanical),
        ("codex".to_string(), false)
    );
    assert_eq!(v.plan_route_id(&settings), "claude-sonnet");
    assert!(v.check_routes(&settings.routes).is_ok());
    v.tier_routes.insert(Tier::Standard, "nope".into());
    let err = v.check_routes(&settings.routes).unwrap_err();
    assert!(err.contains("tierRoutes.standard \"nope\""), "{err}");
    v.tier_routes.clear();
    v.planner_route = Some("gone".into());
    let err = v.check_routes(&settings.routes).unwrap_err();
    assert!(err.contains("plannerRoute \"gone\""), "{err}");
}

#[test]
fn scoped_checks_and_chat_tools_default_to_none() {
    assert!(Settings::default().scoped_checks.is_empty());
    assert!(Settings::default().chat_tools.is_empty());
    let mut json = serde_json::to_value(Settings::default()).unwrap();
    json.as_object_mut().unwrap().remove("scopedChecks");
    json.as_object_mut().unwrap().remove("chatTools");
    let loaded: Settings = serde_json::from_value(json).unwrap();
    assert!(loaded.scoped_checks.is_empty() && loaded.chat_tools.is_empty());
}
