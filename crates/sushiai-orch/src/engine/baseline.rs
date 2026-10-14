//! The baseline diff: a check that fails on the task's branch and on its base
//! is judged by WHICH tests fail, not by its exit code. A red base is normal in
//! other people's repos, so a task that adds no failure of its own passes; one
//! that adds a failure fixes it like any other failing check.

use super::*;
use std::collections::BTreeSet;

/// What the baseline diff decided about one failing check.
pub(super) enum BaseVerdict {
    /// The task's failures are all on the base too (or the output is the same
    /// as the base's): the gate passes. The base's sha is recorded.
    Waived,
    /// The task fails a test the base does not: an ordinary failure.
    /// The task's own run to report (a full run when the first was fail-fast).
    NewFailures(Box<VerifyOutcome>),
    /// Neither output names tests and they differ: the existing question.
    Unclear,
}

/// The pure comparison of one run with the base's run of the same command.
/// A waiver needs strong evidence; anything doubtful is `Unparsed`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Compared {
    /// Nothing fails here that does not fail on the base, every test that
    /// passes on the base passes here, and the rest of the output matches;
    /// the names failing in both.
    Covered(Vec<String>),
    /// Tests failing here and not on the base.
    New(BTreeSet<String>),
    /// Not comparable: no parsed run, a crash, an early stop or
    /// output beyond the test lines that differs.
    Unparsed,
}

pub(super) fn compare(run: &VerifyOutcome, base: &VerifyOutcome) -> Compared {
    // A silent or unparsed run (no stored fingerprint) has nothing to compare.
    if run.output_key.is_empty() || base.output_key.is_empty() {
        return Compared::Unparsed;
    }
    // A run that stopped at its first failing unit never ran the rest.
    if run.stopped_early || base.stopped_early {
        return Compared::Unparsed;
    }
    if run.crashed || base.crashed {
        return Compared::Unparsed;
    }
    if run.failing.is_empty() || base.failing.is_empty() {
        return Compared::Unparsed;
    }
    let (now, before): (BTreeSet<&String>, BTreeSet<&String>) =
        (run.failing.iter().collect(), base.failing.iter().collect());
    let new: BTreeSet<String> = now.difference(&before).map(|n| (*n).clone()).collect();
    if !new.is_empty() {
        return Compared::New(new);
    }
    // An early stop (fail-fast) leaves out tests the base ran and passed.
    let passing: BTreeSet<&String> = run.passing.iter().collect();
    if !base.passing.iter().all(|t| passing.contains(t)) {
        return Compared::Unparsed;
    }
    // Same tests, but something else in the output changed (a new error).
    if run.output_key != base.output_key {
        return Compared::Unparsed;
    }
    Compared::Covered(run.failing.clone())
}

fn waiver_line(base_sha: &str, run: &VerifyOutcome, tests: &[String], flaky: bool) -> String {
    let note = if flaky {
        "; differed between reruns"
    } else {
        ""
    };
    format!(
        "Orchestrator: base failures on {} accepted: {} (check: {}{note})",
        short_sha(base_sha),
        tests.join(", "),
        run.command
    )
}

fn waive(task: &mut Task, run: &VerifyOutcome, tests: &[String], flaky: bool) {
    let line = waiver_line(&task.base_sha, run, tests, flaky);
    if !task.decisions.contains(&line) {
        task.decisions.push(line);
    }
}

/// Judges `failed` (a failing check of this task) against `base_run` (the same
/// command failing on the base). New failures are re-run once in the worktree:
/// failures that change between runs are the environment, not the task, and
/// count as the base's.
pub(super) async fn judge_against_base(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    base_run: &VerifyOutcome,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> BaseVerdict {
    if !(failed.stopped_early || base_run.stopped_early) {
        return judge_pair(app, task, failed, base_run, worktree, run_dir, cancel).await;
    }
    // A fail-fast run is not comparable: take full runs of both sides, or
    // give up (never a waiver).
    let Some(full) = no_fail_fast(&failed.command) else {
        return BaseVerdict::Unclear;
    };
    let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
    let ran = run_verify_commands(
        CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
        worktree,
        &run_dir.join("final"),
        std::slice::from_ref(&full),
        sandbox.as_ref(),
        &task.base_sha,
        cancel,
    )
    .await;
    let (repo, base_sha, id) = (task.repo.clone(), task.base_sha.clone(), task.id.clone());
    let on_base = base_result(
        app,
        Some(&id),
        &repo,
        &base_sha,
        &full,
        &run_dir.join("full-base"),
        cancel,
        true,
    )
    .await;
    let usable = |r: &VerifyOutcome| r.code.is_some_and(|c| c != 0) && !is_interrupted(r);
    match (ran.first(), on_base.as_ref()) {
        (Some(t), Some(b)) if usable(t) && usable(b) && !t.stopped_early && !b.stopped_early => {
            judge_pair(app, task, t, b, worktree, run_dir, cancel).await
        }
        _ => BaseVerdict::Unclear,
    }
}

/// `command` with `--no-fail-fast`, only for a plain `cargo test ...` it is
/// safe to append to.
fn no_fail_fast(command: &str) -> Option<String> {
    let command = command.trim();
    let plain = command == "cargo test" || command.starts_with("cargo test ");
    let shell = command.contains(['&', '|', ';', '<', '>', '$', '`', '(', ')', '"', '\'', '\n']);
    let flags = command
        .split_whitespace()
        .any(|w| matches!(w, "--" | "--no-fail-fast" | "--fail-fast"));
    (plain && !shell && !flags).then(|| format!("{command} --no-fail-fast"))
}

async fn judge_pair(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    base_run: &VerifyOutcome,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> BaseVerdict {
    match compare(failed, base_run) {
        Compared::Covered(tests) => {
            waive(task, failed, &tests, false);
            return BaseVerdict::Waived;
        }
        Compared::Unparsed => return BaseVerdict::Unclear,
        Compared::New(_) => {}
    }
    // New failures: one re-run tells a flaky failure from the task's own. It
    // is flaky only when the re-run passes or shows nothing the base lacks.
    let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
    let again = run_verify_commands(
        CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
        worktree,
        &run_dir.join("final"),
        std::slice::from_ref(&failed.command),
        sandbox.as_ref(),
        &task.base_sha,
        cancel,
    )
    .await;
    let Some(again) = again.first().filter(|r| !is_interrupted(r)) else {
        return BaseVerdict::NewFailures(Box::new(failed.clone()));
    };
    let tests = failed.failing.clone();
    if again.code == Some(0) {
        waive(task, failed, &tests, true);
        return BaseVerdict::Waived;
    }
    match compare(again, base_run) {
        Compared::Covered(_) => {
            waive(task, failed, &tests, true);
            BaseVerdict::Waived
        }
        _ => BaseVerdict::NewFailures(Box::new(failed.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_same_failing_tests_are_covered_and_a_new_one_is_not() {
        let base = run(1, BASE);
        assert!(matches!(
            compare(&run(1, BASE), &base),
            Compared::Covered(t) if t == ["a::bad"]
        ));
        let Compared::New(new) =
            compare(&run(1, &format!("{BASE}test a::worse ... FAILED\n")), &base)
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

    fn fixture(name: &str) -> String {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runner");
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn a_fail_fast_cargo_run_is_never_covered() {
        let base = run(101, &fixture("cargo_base.txt"));
        let task = run(101, &fixture("cargo_task.txt"));
        assert!(base.stopped_early && task.stopped_early);
        assert_eq!(compare(&task, &base), Compared::Unparsed);
    }

    #[test]
    fn only_a_plain_cargo_test_gets_no_fail_fast() {
        assert_eq!(
            no_fail_fast("cargo test --workspace").as_deref(),
            Some("cargo test --workspace --no-fail-fast")
        );
        for no in [
            "cargo test -- --nocapture",
            "cargo build && cargo test",
            "cargo test | tee x",
            "cargo test --no-fail-fast",
            "npm test",
        ] {
            assert_eq!(no_fail_fast(no), None, "{no}");
        }
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
}
