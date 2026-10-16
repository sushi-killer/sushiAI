fn suite(cwd: &std::path::Path, command: &str) -> bool {
    whole_repo_suite(cwd, command)
}

#[test]
fn whole_repo_suite_matches_the_package_manager_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    for c in [
        "npm run ci",
        "npm run test",
        "npm run check",
        "npm test",
        "pnpm ci",
        "pnpm test",
        "pnpm run check",
        "yarn test",
        "yarn run ci",
        "yarn check",
        "make ci",
        "make test",
        "make check",
        "go test ./...",
        "pytest",
        "cd x && npm run ci",
        "true; npm test",
    ] {
        assert!(suite(p, c), "{c}");
    }
}

#[test]
fn whole_repo_suite_matches_cargo_test_without_a_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    assert!(suite(p, "cargo test"));
    assert!(suite(p, "cargo test --workspace"));
    assert!(suite(p, "cargo test -p x --workspace"));
    assert!(suite(p, "cargo test --all-targets -- --nocapture"));
    assert!(!suite(p, "cargo test -p x"));
    assert!(!suite(p, "cargo test --package x"));
    assert!(!suite(p, "cargo test --test lifecycle"));
    assert!(!suite(p, "cargo test --lib"));
    assert!(!suite(p, "cargo test --bin x"));
    assert!(!suite(p, "cargo test some_filter"));
    assert!(suite(p, "cargo nextest run --workspace"));
    assert!(suite(p, "cargo nextest run --no-fail-fast"));
    assert!(!suite(p, "cargo nextest run -p x"));
    assert!(!suite(p, "cargo nextest run -E 'test(a)'"));
    assert!(!suite(p, "cargo nextest run some_filter"));
}

#[test]
fn whole_repo_suite_counts_a_suite_script_that_starts_a_runner_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{
            "test:daemon":"npm run lint:rust && node scripts/test-rust.mjs",
            "test:desktop":"node scripts/test-desktop.mjs",
            "test:contracts":"node --test tests/a.test.cjs tests/b.test.cjs",
            "lint:rust":"cargo fmt --check && node scripts/check-crate-deps.mjs",
            "smoke":"node scripts/smoke.mjs"
        }}"#,
    )
    .unwrap();
    let p = tmp.path();
    assert!(suite(p, "npm run test:daemon"));
    assert!(suite(p, "npm run test:desktop"));
    // Named test files are a targeted check, and only suite scripts count.
    assert!(!suite(p, "npm run test:contracts"));
    assert!(!suite(p, "npm run lint:rust"));
    assert!(!suite(p, "npm run smoke"));
}

#[test]
fn whole_repo_suite_leaves_narrow_commands_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    for c in [
        "npm test -- foo",
        "npm ci",
        "npm run build",
        "pnpm test foo",
        "make build",
        "go test ./pkg/x",
        "go test -run TestX ./...",
        "pytest tests/test_x.py",
        "cargo fmt --check",
        "grep -q x file",
    ] {
        assert!(!suite(p, c), "{c}");
    }
}

#[test]
fn whole_repo_suite_reads_a_package_json_script_one_level_deep() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{
            "test:daemon":"cargo fmt --check && cargo test --workspace",
            "test:one":"cargo test -p sushiai-orch --lib x",
            "wrapper":"npm run test:daemon",
            "lint":"eslint ."
        }}"#,
    )
    .unwrap();
    let p = tmp.path();
    assert!(suite(p, "npm run test:daemon"));
    assert!(suite(p, "pnpm run test:daemon"));
    assert!(!suite(p, "npm run test:one"));
    assert!(!suite(p, "npm run lint"));
    // One level only: the script it calls is not expanded.
    assert!(!suite(p, "npm run wrapper"));
    assert!(!suite(p, "npm run missing"));
    let empty = tempfile::tempdir().unwrap();
    assert!(!suite(empty.path(), "npm run test:daemon"));
}
