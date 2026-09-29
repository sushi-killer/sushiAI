#[test]
fn attempt_screenshots_lists_new_images_under_artifacts_only() {
    let tmp = tempfile::tempdir().unwrap();
    let shots = tmp.path().join("artifacts/ui");
    std::fs::create_dir_all(&shots).unwrap();
    std::fs::write(shots.join("after.png"), b"x").unwrap();
    std::fs::write(shots.join("notes.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("root.png"), b"x").unwrap();
    let found = attempt_screenshots(tmp.path(), 0);
    assert_eq!(found, vec![shots.join("after.png")]);
    assert!(attempt_screenshots(tmp.path(), i64::MAX).is_empty());
}

#[test]
fn glob_match_supports_double_star_suffix() {
    assert!(glob_match("src/app/**", "src/app/SectionPage.tsx"));
    assert!(glob_match("src/app/**", "src/app/nested/Deep.tsx"));
    assert!(!glob_match("src/app/**", "src/extensions/registry.ts"));
    assert!(glob_match("*.md", "README.md"));
    assert!(!glob_match("*.md", "README.txt"));
}

#[test]
fn matches_any_protected_checks_every_glob() {
    let globs = vec!["src/app/**".to_string(), "electron/main.cjs".to_string()];
    assert!(matches_any_protected("src/app/SectionPage.tsx", &globs));
    assert!(matches_any_protected("electron/main.cjs", &globs));
    assert!(!matches_any_protected("src/extensions/registry.ts", &globs));
}

fn with_route(settings: &mut Settings, id: &str, harness: Harness, model: &str, strength: Option<u32>) {
    settings.routes.push(Route {
        id: id.into(),
        label: id.into(),
        harness,
        model: Some(model.into()),
        effort: None,
        profile_id: None,
        strength,
    });
}

fn find<'a>(settings: &'a Settings, id: &str) -> &'a Route {
    settings.routes.iter().find(|r| r.id == id).unwrap()
}

#[test]
fn review_auto_mechanical_luna_is_reviewed_by_sonnet() {
    let mut settings = Settings::default();
    with_route(&mut settings, "codex-luna", Harness::Codex, "gpt-5.6-luna", None);
    let luna = find(&settings, "codex-luna");
    let (route, reason) = select_review_route(&settings, luna, Tier::Mechanical).unwrap();
    assert_eq!(route.id, "claude-sonnet");
    assert_eq!(reason, "mechanical tier, strength 2");
}

#[test]
fn review_auto_standard_sonnet_is_reviewed_by_sonnet() {
    let settings = Settings::default();
    let sonnet = find(&settings, "claude-sonnet");
    let (route, _) = select_review_route(&settings, sonnet, Tier::Standard).unwrap();
    assert_eq!(route.id, "claude-sonnet");
}

#[test]
fn review_auto_prefers_a_cheaper_route_and_the_other_harness_on_a_tie() {
    let mut settings = Settings::default();
    settings.prices.insert(
        "gpt-mid".into(),
        Price { input: 1.0, cached_input: 0.1, output: 5.0, cache_write: None },
    );
    with_route(&mut settings, "codex-mid", Harness::Codex, "gpt-mid", Some(2));
    let sonnet = find(&settings, "claude-sonnet");
    let (route, _) = select_review_route(&settings, sonnet, Tier::Standard).unwrap();
    assert_eq!(route.id, "codex-mid", "cheaper at strength 2");

    // Exactly equal price: the other harness beats the implementer's own.
    settings.prices.insert(
        "gpt-mid".into(),
        Price { input: 2.0, cached_input: 0.1, output: 10.0, cache_write: None },
    );
    let sonnet = find(&settings, "claude-sonnet");
    let (route, _) = select_review_route(&settings, sonnet, Tier::Standard).unwrap();
    assert_eq!(route.id, "codex-mid");
}

#[test]
fn review_auto_hard_tier_is_reviewed_by_opus_even_for_a_weaker_implementer() {
    let settings = Settings::default();
    let sonnet = find(&settings, "claude-sonnet");
    let (route, reason) = select_review_route(&settings, sonnet, Tier::Hard).unwrap();
    assert_eq!(route.id, "claude-opus");
    assert_eq!(reason, "hard tier, strength 3");
}

#[test]
fn review_auto_without_a_strong_route_uses_the_strongest() {
    let mut settings = Settings::default();
    settings.routes.retain(|r| r.id != "claude-opus");
    let sonnet = find(&settings, "claude-sonnet");
    let (route, reason) = select_review_route(&settings, sonnet, Tier::Hard).unwrap();
    assert_eq!(route.id, "claude-sonnet");
    assert_eq!(reason, "no route at strength 3; strongest available");
}

#[test]
fn route_strength_defaults_and_overrides() {
    let mut settings = Settings::default();
    for (id, model, want) in [
        ("h", "claude-haiku-4-5", 1),
        ("l", "gpt-5.6-Luna", 1),
        ("m", "gpt-5-mini", 1),
        ("s", "sonnet", 2),
        ("g", "gpt-5.3-codex", 2),
        ("o", "opus", 3),
    ] {
        with_route(&mut settings, id, Harness::Claude, model, None);
        assert_eq!(route_strength(find(&settings, id)), want, "{model}");
    }
    assert_eq!(route_strength(find(&settings, "codex")), 2, "no model");
    with_route(&mut settings, "x", Harness::Claude, "haiku", Some(3));
    assert_eq!(route_strength(find(&settings, "x")), 3);
}

#[test]
fn settings_without_route_strength_still_load() {
    let route: Route =
        serde_json::from_str(r#"{"id":"a","label":"A","harness":"claude"}"#).unwrap();
    assert_eq!(route.strength, None);
    assert!(!serde_json::to_string(&route).unwrap().contains("strength"));
    let route: Route =
        serde_json::from_str(r#"{"id":"a","label":"A","harness":"claude","strength":3}"#).unwrap();
    assert_eq!(route.strength, Some(3));
}

#[test]
fn select_review_route_explicit_id() {
    let settings = Settings {
        review: "claude-haiku".to_string(),
        ..Settings::default()
    };
    let implementer = find(&settings, "claude-opus");
    let (route, reason) = select_review_route(&settings, implementer, Tier::Hard).unwrap();
    assert_eq!(route.id, "claude-haiku", "even weaker than the implementer");
    assert_eq!(reason, "explicit setting");
}
