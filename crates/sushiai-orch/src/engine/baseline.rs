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
    NewFailures,
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
    /// Not comparable: no parsed run, a crash, a cut list, an early stop or
    /// output beyond the test lines that differs.
    Unparsed,
}

pub(super) fn compare(run: &VerifyOutcome, base: &VerifyOutcome) -> Compared {
    // A silent or unparsed run (no stored fingerprint) has nothing to compare.
    if run.output_key.is_empty() || base.output_key.is_empty() {
        return Compared::Unparsed;
    }
    if run.truncated || base.truncated || run.crashed || base.crashed {
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
        return BaseVerdict::NewFailures;
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
        _ => BaseVerdict::NewFailures,
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
            truncated: report.truncated,
            crashed: report.crashed,
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
    fn a_cut_list_is_not_comparable() {
        let many: String = (0..600).map(|i| format!("not ok {i} - t{i}\n")).collect();
        let base = run(1, &many);
        let task = run(1, &format!("{many}not ok 900 - new\n"));
        assert!(base.truncated);
        assert_eq!(compare(&task, &base), Compared::Unparsed);
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
}
