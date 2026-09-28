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

#[test]
fn select_review_route_auto_prefers_the_hard_tier_then_another_harness() {
    let settings = Settings::default();
    let route = |id: &str| settings.routes.iter().find(|r| r.id == id).unwrap();
    let hard = settings.tiers.get(&Tier::Hard).unwrap();
    assert_eq!(
        &select_review_route(&settings, route("claude-sonnet"), false)
            .unwrap()
            .id,
        hard
    );
    assert_eq!(
        &select_review_route(&settings, route("codex"), false)
            .unwrap()
            .id,
        hard
    );
    let for_hard = select_review_route(&settings, route(hard), false).unwrap();
    assert_ne!(for_hard.harness, route(hard).harness);
}

#[test]
fn select_review_route_other_family_never_reviews_claude_with_claude() {
    let settings = Settings::default();
    let route = |id: &str| settings.routes.iter().find(|r| r.id == id).unwrap();
    let for_sonnet = select_review_route(&settings, route("claude-sonnet"), true).unwrap();
    assert_eq!(for_sonnet.harness, Harness::Codex);
    let for_codex = select_review_route(&settings, route("codex"), true).unwrap();
    assert_eq!(
        for_codex.id, "claude-opus",
        "the hard route when it is the other family"
    );

    // A hard route on Codex, listed after another Codex route, still wins.
    let mut settings = Settings::default();
    settings.routes.push(Route {
        id: "codex-strong".into(),
        label: "Codex strong".into(),
        harness: Harness::Codex,
        model: Some("gpt-5.3-codex".into()),
        effort: None,
        profile_id: None,
    });
    settings.tiers.insert(Tier::Hard, "codex-strong".into());
    let sonnet = settings
        .routes
        .iter()
        .find(|r| r.id == "claude-sonnet")
        .unwrap();
    assert_eq!(
        select_review_route(&settings, sonnet, true).unwrap().id,
        "codex-strong"
    );
}

#[test]
fn select_review_route_explicit_id() {
    let settings = Settings {
        review: "claude-opus".to_string(),
        ..Settings::default()
    };
    let implementer = settings.routes.iter().find(|r| r.id == "codex").unwrap();
    let route = select_review_route(&settings, implementer, false).unwrap();
    assert_eq!(route.id, "claude-opus");
}
