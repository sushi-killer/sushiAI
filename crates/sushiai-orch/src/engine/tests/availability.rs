fn only(harness: Harness) -> impl Fn(Harness) -> bool {
    move |h| h == harness
}

fn find_route(settings: &Settings, id: &str) -> Route {
    settings.routes.iter().find(|r| r.id == id).unwrap().clone()
}

#[test]
fn a_missing_harness_falls_to_the_same_strength_then_stronger_then_weaker() {
    let mut settings = Settings::default();
    // codex (strength 2) is the wanted route; only claude is installed.
    let codex = find_route(&settings, "codex");
    let got = nearest_available(&settings, only(Harness::Claude), &codex).unwrap();
    assert_eq!(got.id, "claude-sonnet", "same strength wins over stronger");

    // Without a strength-2 claude route, the weakest stronger one is next.
    settings.routes.retain(|r| r.id != "claude-sonnet");
    let got = nearest_available(&settings, only(Harness::Claude), &codex).unwrap();
    assert_eq!(got.id, "claude-opus");

    // With only a weaker one left, that one is still used ("any").
    let mut weak = find_route(&Settings::default(), "claude-sonnet");
    weak.id = "claude-weak".into();
    weak.strength = Some(1);
    settings.routes = vec![codex.clone(), weak];
    let got = nearest_available(&settings, only(Harness::Claude), &codex).unwrap();
    assert_eq!(got.id, "claude-weak");
}

#[test]
fn an_available_route_is_kept_and_no_available_harness_finds_nothing() {
    let settings = Settings::default();
    let codex = find_route(&settings, "codex");
    assert_eq!(
        nearest_available(&settings, |_| true, &codex).unwrap().id,
        "codex"
    );
    assert!(nearest_available(&settings, |_| false, &codex).is_none());
}

#[test]
fn the_cheapest_available_route_ignores_routes_on_a_missing_harness() {
    let settings = Settings::default();
    let got = cheapest_available(&settings, only(Harness::Claude)).unwrap();
    assert_eq!(got.harness, Harness::Claude);
    assert!(cheapest_available(&settings, |_| false).is_none());
}

#[test]
fn a_binary_path_is_found_only_when_it_is_an_executable_file() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    std::fs::write(&plain, "x").unwrap();
    assert!(!binary_found(plain.to_str().unwrap()));
    assert!(!binary_found(dir.path().join("absent").to_str().unwrap()));
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(binary_found(plain.to_str().unwrap()));
}

#[test]
fn the_hook_budget_follows_the_verify_timeout_under_the_cap() {
    assert_eq!(hook_budget_secs(30), 30);
    assert_eq!(hook_budget_secs(540), 540);
    assert_eq!(hook_budget_secs(1200), 540);
    assert_eq!(hook_budget_secs(0), 1);
}

#[test]
fn a_timeout_message_names_the_configured_limit() {
    assert_eq!(
        timed_out_tail(Duration::from_secs(1200)),
        "timed out after 20 minutes"
    );
    assert_eq!(
        timed_out_tail(Duration::from_secs(1)),
        "timed out after 1 seconds"
    );
}

fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    dir
}

const PKG: &str = r#"{"scripts": {"build": "x", "test": "y"}}"#;

#[test]
fn suggested_verify_commands_use_the_lockfile_runner() {
    for (lock, want) in [
        ("pnpm-lock.yaml", "pnpm run test"),
        ("yarn.lock", "yarn run test"),
        ("bun.lockb", "bun run test"),
        ("bun.lock", "bun run test"),
        ("package-lock.json", "npm run test"),
    ] {
        let dir = repo_with(&[("package.json", PKG), (lock, "")]);
        assert_eq!(verify_options(dir.path())[0], want, "{lock}");
    }
    let dir = repo_with(&[("package.json", PKG)]);
    assert_eq!(
        verify_options(dir.path()),
        ["npm run test", "npm run build"]
    );
}

#[test]
fn suggested_verify_commands_cover_other_toolchains() {
    let dir = repo_with(&[
        ("Cargo.toml", ""),
        ("go.mod", ""),
        ("pyproject.toml", ""),
        ("Makefile", "build:\n\tcc\n\ntest: build\n\t./t\n"),
    ]);
    assert_eq!(
        verify_options(dir.path()),
        ["cargo test", "go test ./...", "pytest", "make test"]
    );
    let dir = repo_with(&[("pytest.ini", ""), ("Makefile", "build:\n\tcc\ntesting:\n")]);
    assert_eq!(verify_options(dir.path()), ["pytest"]);
    assert!(verify_options(repo_with(&[]).path()).is_empty());
}

#[test]
fn a_command_check_names_any_common_test_runner() {
    for tool in [
        "bun test",
        "pnpm test",
        "yarn test",
        "gradle test",
        "./gradlew test",
        "mvn verify",
        "dotnet test",
        "uv run pytest",
        "rspec",
        "go test ./...",
        "cargo test",
        "make check",
    ] {
        let criterion = format!("x -- check: {tool}");
        assert!(is_command_check(&criterion), "{tool}");
    }
    assert!(!is_command_check("x -- check: read the makefile"));
}
