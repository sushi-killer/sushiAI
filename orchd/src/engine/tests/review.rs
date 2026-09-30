#[test]
fn attempt_screenshots_lists_new_images_under_artifacts_only() {
    let tmp = tempfile::tempdir().unwrap();
    let shots = tmp.path().join("artifacts/ui");
    std::fs::create_dir_all(&shots).unwrap();
    std::fs::write(shots.join("after.png"), b"x").unwrap();
    std::fs::write(shots.join("notes.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("root.png"), b"x").unwrap();
    let found = attempt_screenshots(tmp.path(), 0, &EvidenceScope::All);
    assert_eq!(found, vec![shots.join("after.png")]);
    assert!(attempt_screenshots(tmp.path(), i64::MAX, &EvidenceScope::All).is_empty());
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
        review: "claude-sonnet".to_string(),
        ..Settings::default()
    };
    let implementer = find(&settings, "claude-opus");
    let (route, reason) = select_review_route(&settings, implementer, Tier::Hard).unwrap();
    assert_eq!(route.id, "claude-sonnet", "even weaker than the implementer");
    assert_eq!(reason, "explicit setting");
}

fn visual_task(criteria: &[&str]) -> Task {
    let mut task = task_with_status(TaskStatus::Running);
    task.criteria = criteria.iter().map(|c| c.to_string()).collect();
    task
}

fn write_images(root: &Path, names: &[&str]) {
    for name in names {
        let path = root.join("artifacts").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
}

#[test]
fn evidence_scope_names_the_files_and_directories_visual_criteria_mention() {
    let task = visual_task(&[
        "panel shows a title -- check: screenshot artifacts/panel.png",
        "list is sorted -- check: screenshots under artifacts/list/ look right",
        "npm test passes -- check: npm test",
    ]);
    assert_eq!(
        evidence_scope(&task),
        EvidenceScope::Named(vec![
            "artifacts/panel.png".to_string(),
            "artifacts/list/*".to_string()
        ])
    );
}

#[test]
fn evidence_scope_is_all_when_a_visual_criterion_names_no_path_and_nothing_without_one() {
    let bare = visual_task(&[
        "panel -- check: screenshot artifacts/panel.png",
        "other -- check: a screenshot under artifacts/",
    ]);
    assert_eq!(evidence_scope(&bare), EvidenceScope::All);
    let unnamed = visual_task(&["looks right -- check: screenshot"]);
    assert_eq!(evidence_scope(&unnamed), EvidenceScope::All);
    let none = visual_task(&["it works -- check: cargo test"]);
    assert_eq!(evidence_scope(&none), EvidenceScope::Nothing);
}

#[test]
fn evidence_copies_only_the_named_images() {
    let tmp = tempfile::tempdir().unwrap();
    write_images(
        tmp.path(),
        &["panel.png", "workspace.png", "smoke-hidden-window.png", "list/a.png"],
    );
    let task = visual_task(&["panel -- check: screenshot artifacts/panel.png"]);
    let scope = evidence_scope(&task);
    let saved = save_evidence(tmp.path(), 0, &tmp.path().join("ev"), &scope);
    let names: Vec<_> = saved.iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["panel.png"]);
    let shown = attempt_screenshots(tmp.path(), 0, &scope);
    assert_eq!(shown, vec![tmp.path().join("artifacts/panel.png")]);
    assert!(scope.missing(&saved).is_empty());
    assert!(scope.allows_saved("/x/runs/1/evidence/panel.png"));
    assert!(!scope.allows_saved("/x/runs/1/evidence/workspace.png"));
}

#[test]
fn a_named_image_the_attempt_did_not_write_is_missing_from_the_gate() {
    let tmp = tempfile::tempdir().unwrap();
    write_images(tmp.path(), &["workspace.png", "smoke-hidden-window.png"]);
    let task = visual_task(&["panel -- check: screenshot artifacts/panel.png"]);
    let scope = evidence_scope(&task);
    let saved = save_evidence(tmp.path(), 0, &tmp.path().join("ev"), &scope);
    assert!(saved.is_empty());
    assert_eq!(scope.missing(&saved), vec!["artifacts/panel.png"]);
}

#[test]
fn a_directory_a_check_names_is_evidence_for_every_image_under_it() {
    let tmp = tempfile::tempdir().unwrap();
    write_images(tmp.path(), &["list/a.png", "list/deep/b.png", "workspace.png"]);
    let task = visual_task(&["sorted -- check: screenshots under artifacts/list/"]);
    let scope = evidence_scope(&task);
    let saved = save_evidence(tmp.path(), 0, &tmp.path().join("ev"), &scope);
    assert_eq!(saved.len(), 2);
    assert!(scope.missing(&saved).is_empty());
    assert_eq!(scope.missing(&[]), vec!["artifacts/list/"]);
}

#[test]
fn an_unscoped_visual_criterion_and_a_task_without_one_behave_as_named() {
    let tmp = tempfile::tempdir().unwrap();
    write_images(tmp.path(), &["a.png", "b.png"]);
    let all = evidence_scope(&visual_task(&["looks right -- check: screenshot"]));
    assert_eq!(save_evidence(tmp.path(), 0, &tmp.path().join("ev"), &all).len(), 2);
    let none = evidence_scope(&visual_task(&["x -- check: cargo test"]));
    assert!(save_evidence(tmp.path(), 0, &tmp.path().join("ev2"), &none).is_empty());
    assert!(attempt_screenshots(tmp.path(), 0, &none).is_empty());
}
