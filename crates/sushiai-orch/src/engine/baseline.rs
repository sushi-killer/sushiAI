//! The baseline diff: a check that fails on the task's branch and on its base
//! is judged by WHICH tests fail, not by its exit code. A red base is normal in
//! other people's repos, so a task that adds no failure of its own passes; one
//! that adds a failure fixes it like any other failing check.

use super::*;
use crate::test_failures::{failing_tests, output_key};
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
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Compared {
    /// Nothing fails here that does not fail on the base; the names failing
    /// in both (empty when the outputs were identical but unparsed).
    Covered(Vec<String>),
    /// Tests failing here and not on the base.
    New(BTreeSet<String>),
    /// Not comparable: an output no runner format explains, and it differs.
    Unparsed,
}

fn failing_of(outcome: &VerifyOutcome) -> BTreeSet<String> {
    if outcome.failing.is_empty() {
        failing_tests(&outcome.tail)
    } else {
        outcome.failing.iter().cloned().collect()
    }
}

fn key_of(outcome: &VerifyOutcome) -> String {
    if outcome.output_key.is_empty() {
        output_key(&outcome.tail)
    } else {
        outcome.output_key.clone()
    }
}

pub(super) fn compare(run: &VerifyOutcome, base: &VerifyOutcome) -> Compared {
    let (now, before) = (failing_of(run), failing_of(base));
    if !now.is_empty() && !before.is_empty() {
        let new: BTreeSet<String> = now.difference(&before).cloned().collect();
        return if new.is_empty() {
            Compared::Covered(now.into_iter().collect())
        } else {
            Compared::New(new)
        };
    }
    // A silent failure has nothing to compare: it stays a question.
    if run.code == base.code && !run.tail.trim().is_empty() && key_of(run) == key_of(base) {
        return Compared::Covered(Vec::new());
    }
    Compared::Unparsed
}

fn waiver_line(base_sha: &str, run: &VerifyOutcome, tests: &[String], flaky: bool) -> String {
    let accepted = if tests.is_empty() {
        format!(
            "output identical to the base's (exit {})",
            run.code.unwrap_or(-1)
        )
    } else {
        tests.join(", ")
    };
    let note = if flaky {
        "; differed between reruns"
    } else {
        ""
    };
    format!(
        "Orchestrator: base failures on {} accepted: {accepted} (check: {}{note})",
        short_sha(base_sha),
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
    let first = match compare(failed, base_run) {
        Compared::Covered(tests) => {
            waive(task, failed, &tests, false);
            return BaseVerdict::Waived;
        }
        Compared::Unparsed => return BaseVerdict::Unclear,
        Compared::New(new) => new,
    };
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
    if again.code == Some(0) {
        waive(task, failed, &[], true);
        return BaseVerdict::Waived;
    }
    match compare(again, base_run) {
        Compared::Covered(tests) => {
            waive(task, failed, &tests, true);
            BaseVerdict::Waived
        }
        Compared::New(second) if first.is_disjoint(&second) => {
            let tests: Vec<String> = failing_of(failed).into_iter().collect();
            waive(task, failed, &tests, true);
            BaseVerdict::Waived
        }
        _ => BaseVerdict::NewFailures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: i32, tail: &str) -> VerifyOutcome {
        VerifyOutcome {
            command: "check".into(),
            code: Some(code),
            tail: tail.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_same_failing_tests_are_covered_and_a_new_one_is_not() {
        let base = run(1, "test a ... FAILED\ntest b ... FAILED");
        assert!(matches!(
            compare(&run(1, "test b ... FAILED"), &base),
            Compared::Covered(t) if t == ["b"]
        ));
        let Compared::New(new) = compare(&run(1, "test b ... FAILED\ntest c ... FAILED"), &base)
        else {
            panic!("a new failure must be reported");
        };
        assert_eq!(new.into_iter().collect::<Vec<_>>(), ["c"]);
    }

    #[test]
    fn unparsed_outputs_compare_by_code_and_digit_blind_text() {
        let base = run(2, "lint: bad file after 12ms");
        assert_eq!(
            compare(&run(2, "lint: bad file after 7ms"), &base),
            Compared::Covered(Vec::new())
        );
        assert_eq!(compare(&run(2, "lint: other"), &base), Compared::Unparsed);
        assert_eq!(
            compare(&run(3, "lint: bad file after 1ms"), &base),
            Compared::Unparsed
        );
    }
}
