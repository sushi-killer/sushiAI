use super::{compare, Compared};
use crate::task::attempt::VerifyOutcome;
fn run(code: i32, output: &str) -> VerifyOutcome {
    let report = crate::test_failures::parse(output, "");
    VerifyOutcome {
        command: "check".into(),
        code: Some(code),
        tail: output.into(),
        failing: report.failing,
        passing: report.passing,
        crashed: report.crashed,
        stopped_early: report.stopped_early,
        output_key: report.rest_key,
        ..Default::default()
    }
}
const BASE: &str = "test a::one ... ok\ntest a::two ... ok\ntest a::bad ... FAILED\n";
fn fixture(name: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runner");
    std::fs::read_to_string(dir.join(name)).unwrap()
}
#[test]
fn the_same_failing_tests_are_covered_and_a_new_one_is_not() {
    let base = run(1, BASE);
    assert!(matches!(
        compare(&run(1, BASE), &base),
        Compared::Covered(t) if t == ["a::bad"]
    ));
    let Compared::New(new) = compare(&run(1, &format!("{BASE}test a::worse ... FAILED\n")), &base)
    else {
        panic!("a new failure must be reported");
    };
    assert_eq!(new.into_iter().collect::<Vec<_>>(), ["a::worse"]);
}

#[test]
fn a_fail_fast_run_that_skips_tests_the_base_passed_is_not_covered() {
    let base = run(1, BASE);
    let early = run(1, "test a::bad ... FAILED\n");
    assert_eq!(compare(&early, &base), Compared::Unparsed);
}

#[test]
fn a_crash_or_a_red_build_is_never_covered() {
    let base = run(1, BASE);
    let crash = run(
        1,
        &format!("{BASE}process didn't exit successfully (signal: 11, SIGSEGV)\n"),
    );
    assert_eq!(compare(&crash, &base), Compared::Unparsed);
    let build = run(101, "error: could not compile `x`\n");
    assert_eq!(compare(&build, &build), Compared::Unparsed);
}

#[test]
fn an_identical_fingerprint_alone_is_never_covered() {
    let base = run(1, "coverage 85.2% is below the gate\n");
    let task = run(1, "coverage 12.4% is below the gate\n");
    assert_eq!(compare(&task, &base), Compared::Unparsed);
    assert_eq!(compare(&base, &base), Compared::Unparsed);
}

#[test]
fn the_stored_outcome_has_counts_and_a_key_and_the_lists_come_back_from_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let passing: String = (0..600).map(|i| format!("ok {i} - t{i}\n")).collect();
    let output = format!("{passing}not ok 600 - bad\n");
    let mut base = run(1, &output);
    base.tail.clear();
    crate::test_failures::stash_lists(dir.path(), &mut base);
    assert_eq!((base.failing_count, base.passing_count), (1, 600));
    assert_eq!(base.tests_key.len(), 16);
    let stored = serde_json::to_string(&base).unwrap();
    assert!(!stored.contains("t599"), "{} bytes", stored.len());
    assert!(stored.len() < 1000, "{} bytes", stored.len());
    let mut loaded: VerifyOutcome = serde_json::from_str(&stored).unwrap();
    let task = run(1, &output);
    assert_eq!(compare(&task, &loaded), Compared::Unparsed);
    crate::test_failures::hydrate_lists(dir.path(), &mut loaded);
    assert_eq!(
        compare(&task, &loaded),
        Compared::Covered(vec!["bad".into()])
    );
}

#[test]
fn a_suite_of_600_passing_tests_with_a_base_identical_failure_is_covered() {
    let passing: String = (0..600).map(|i| format!("ok {i} - t{i}\n")).collect();
    let output = format!("{passing}not ok 600 - bad\n");
    let base = run(1, &output);
    let task = run(1, &output);
    assert_eq!(compare(&task, &base), Compared::Covered(vec!["bad".into()]));
    let new = run(1, &format!("{output}not ok 601 - new\n"));
    assert!(matches!(compare(&new, &base), Compared::New(_)));
    let lost = run(1, "not ok 600 - bad\n");
    assert_eq!(compare(&lost, &base), Compared::Unparsed);
}

#[test]
fn other_output_that_changed_is_not_covered() {
    let base = run(1, "ERROR connecting to db\nnot ok 1 - alpha\n");
    let task = run(
        1,
        "ERROR connecting to db\nsrc/a.ts(3,1): error TS2322: no\nnot ok 1 - alpha\n",
    );
    assert_eq!(compare(&task, &base), Compared::Unparsed);
}

#[test]
fn a_fail_fast_cargo_run_is_never_covered() {
    let base = run(101, &fixture("cargo_base.txt"));
    let task = run(101, &fixture("cargo_task.txt"));
    assert!(base.stopped_early && task.stopped_early);
    assert_eq!(compare(&task, &base), Compared::Unparsed);
}

#[test]
fn a_node_spec_red_base_is_covered_even_with_other_timings() {
    let text = fixture("node_spec.txt");
    let base = run(1, &text);
    assert_eq!(base.failing, ["adds"]);
    assert_eq!(base.passing, ["ok one"]);
    let later = text
        .replace("0.898ms", "1.4ms")
        .replace("71.944917", "80.1");
    assert!(matches!(
        compare(&run(1, &later), &base),
        Compared::Covered(_)
    ));
}

#[test]
fn runs_that_differ_only_in_timing_are_covered() {
    let a = "\u{2713} src/a.test.ts (2 tests) 3ms\n FAIL  src/b.test.ts > suite > case\n";
    let b = "\u{2713} src/a.test.ts (2 tests) 12ms\n FAIL  src/b.test.ts > suite > case\n";
    let (base, task) = (run(1, a), run(1, b));
    assert_eq!(base.passing, ["src/a.test.ts"]);
    assert!(matches!(compare(&task, &base), Compared::Covered(_)));
}
