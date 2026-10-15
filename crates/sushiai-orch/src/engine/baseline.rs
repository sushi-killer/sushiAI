//! The baseline diff: a check that fails on the task's branch and on its base
//! is judged by WHICH tests fail, not by its exit code. A red base is normal in
//! other people's repos, so a task that adds no failure of its own passes; one
//! that adds a failure fixes it like any other failing check.

#[cfg(test)]
use crate::task::TaskStatus;

use crate::task::questions::{WAIVED, WAIVER_BY};

use crate::verify::comparison::{compare, Compared};

use crate::engine::attribution::attribute;
use crate::engine::state::App;
use crate::engine::verification::base::checks::base_result;
use crate::engine::verification::base::checks::base_rev;
use crate::engine::verification::runner::{run_verify_commands, CheckRunner};
use crate::git::paths::short_sha;
use crate::task::Assumption;
use crate::task::Blame;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::VerifyOutcome;
use crate::verify::passes::is_interrupted;
use std::path::Path;
use std::sync::Arc;

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

/// The same check observed on the task tree and its base.
#[derive(Clone, Copy)]
pub(in crate::engine) struct Comparison<'a> {
    pub(in crate::engine) failed: &'a VerifyOutcome,
    pub(in crate::engine) base: &'a VerifyOutcome,
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
    accept_waiver(task, run, waiver_line(&task.base_sha, run, tests, flaky));
}

fn waive_rerun(task: &mut Task, run: &VerifyOutcome) {
    accept_waiver(
        task,
        run,
        format!(
            "Orchestrator: flaky check {} accepted on {}: {}; passed on rerun",
            run.command,
            short_sha(&task.base_sha),
            run.failing.join(", ")
        ),
    );
}

fn accept_waiver(task: &mut Task, run: &VerifyOutcome, line: String) {
    if !task.decisions.contains(&line) {
        task.record_decision(line.clone());
    }
    if !task.assumptions.iter().any(|assumption| {
        assumption.by == WAIVER_BY
            && !assumption.overturned
            && assumption.commands == [run.command.clone()]
            && assumption.evidence == line
    }) {
        task.record_assumption(Assumption {
            question: format!(
                "{} failed: accept the observed base or rerun evidence?",
                run.command
            ),
            answer: WAIVED.into(),
            evidence: line,
            by: WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: task.attempts.last().map(|attempt| attempt.n),
            commands: vec![run.command.clone()],
            overturned: false,
            owner_answer: None,
        });
    }
}

/// Applies the same baseline comparison to an observed failing delivery check.
pub(in crate::engine) async fn judge_check(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    run_dir: &Path,
    runner: CheckRunner<'_>,
) -> BaseVerdict {
    let base = match base_rev(task) {
        Ok(rev) => {
            base_result(
                app,
                crate::engine::verification::base::jobs::BaseCheckJob::new(
                    Some(&task.id),
                    &task.repo,
                    &rev,
                    &run_dir.join("carry-base"),
                    true,
                ),
                &failed.command,
                runner.cancel,
            )
            .await
        }
        Err(_) => None,
    };
    if let Some(base) = base.filter(|base| !is_interrupted(base)) {
        return judge_against_base(
            app,
            task,
            Comparison {
                failed,
                base: &base,
            },
            run_dir,
            runner,
        )
        .await;
    }
    let again = run_verify_commands(runner, std::slice::from_ref(&failed.command)).await;
    if !runner.cancel.is_cancelled() && again.first().is_some_and(|run| run.code == Some(0)) {
        waive_rerun(task, failed);
        BaseVerdict::Waived
    } else {
        BaseVerdict::NewFailures(Box::new(failed.clone()))
    }
}

/// Judges `failed` (a failing check of this task) against `base_run` (the same
/// command failing on the base). New failures are re-run once in the worktree:
/// failures that change between runs are the environment, not the task, and
/// count as the base's.
pub(super) async fn judge_against_base(
    app: &Arc<App>,
    task: &mut Task,
    comparison: Comparison<'_>,
    run_dir: &Path,
    runner: CheckRunner<'_>,
) -> BaseVerdict {
    let Comparison {
        failed,
        base: base_run,
    } = comparison;
    if !(failed.stopped_early || base_run.stopped_early) {
        return judge_pair(app, task, comparison, runner).await;
    }
    // A fail-fast run is not comparable: take full runs of both sides, or
    // give up (never a waiver).
    let Some(full) = no_fail_fast(&failed.command) else {
        return settle_unclear(app, task, comparison, runner).await;
    };
    let ran = run_verify_commands(runner, std::slice::from_ref(&full)).await;
    // The commit the other final base checks run on.
    let (repo, id) = (task.repo.clone(), task.id.clone());
    let Ok(rev) = base_rev(task) else {
        return settle_unclear(app, task, comparison, runner).await;
    };
    let on_base = base_result(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&id),
            &repo,
            &rev,
            &run_dir.join("full-base"),
            true,
        ),
        &full,
        runner.cancel,
    )
    .await;
    let usable = |r: &VerifyOutcome| r.code.is_some() && !is_interrupted(r);
    match (ran.first(), on_base.as_ref()) {
        (Some(run), _) if run.code == Some(0) && !runner.cancel.is_cancelled() => {
            waive(task, failed, &failed.failing, true);
            BaseVerdict::Waived
        }
        (Some(t), Some(b)) if usable(t) && usable(b) && !t.stopped_early && !b.stopped_early => {
            judge_pair(app, task, Comparison { failed: t, base: b }, runner).await
        }
        _ => settle_unclear(app, task, comparison, runner).await,
    }
}

/// Reads a pair the comparison cannot settle with one attribution turn. A
/// turn that gives no valid verdict leaves the question to the owner.
/// `introduced`, or a check the task is meant to turn green, is an ordinary
/// failure; a verified `inherited` waives it as an assumption the owner can
/// overturn. `environment` and `incomplete` first rerun the check once on the
/// task tree; only a verdict that is still one of them stays a question.
async fn settle_unclear(
    app: &Arc<App>,
    task: &mut Task,
    comparison: Comparison<'_>,
    runner: CheckRunner<'_>,
) -> BaseVerdict {
    let Comparison {
        failed,
        base: base_run,
    } = comparison;
    // ponytail: the turn reads the diff and both runs but cannot run
    // commands; it should become a sandboxed, command-capable read-only turn
    // that reproduces the narrowest failing piece on the base itself.
    let Some(mut att) = attribute(app, task, comparison, runner).await else {
        return BaseVerdict::Unclear;
    };
    let mut run = failed.clone();
    if !att.target && matches!(att.cause, Blame::Environment | Blame::Incomplete) {
        let again = run_verify_commands(runner, std::slice::from_ref(&failed.command)).await;
        let again = again.into_iter().next();
        // A rerun that timed out is a code-checked environment signal.
        let rerun_timed_out = again.as_ref().is_some_and(is_interrupted);
        let Some(again) = again.filter(|r| !is_interrupted(r)) else {
            let cause = typed_cause(att.cause, rerun_timed_out || environmental(failed));
            set_cause(task, cause);
            return BaseVerdict::Unclear;
        };
        if again.code == Some(0) {
            // The failure did not come back: it was the environment's.
            set_cause(task, None);
            waive(task, failed, &failed.failing, true);
            return BaseVerdict::Waived;
        }
        match compare(&again, base_run) {
            Compared::Covered(tests) => {
                set_cause(task, None);
                waive(task, &again, &tests, true);
                return BaseVerdict::Waived;
            }
            Compared::New(_) => {
                set_cause(task, Some(Blame::Introduced));
                return BaseVerdict::NewFailures(Box::new(again));
            }
            Compared::Unparsed => {}
        }
        let Some(second) = attribute(
            app,
            task,
            Comparison {
                failed: &again,
                base: base_run,
            },
            runner,
        )
        .await
        else {
            let cause = typed_cause(att.cause, environmental(&again));
            set_cause(task, cause);
            return BaseVerdict::Unclear;
        };
        att = second;
        run = again;
    }
    let cause = typed_cause(att.cause, environmental(&run));
    set_cause(task, cause);
    if att.target || att.cause == Blame::Introduced {
        return BaseVerdict::NewFailures(Box::new(run));
    }
    if att.cause != Blame::Inherited {
        return BaseVerdict::Unclear;
    }
    waive(task, &run, &att.pre_existing, false);
    let commands = vec![run.command.clone()];
    if !task
        .assumptions
        .iter()
        .any(|a| a.commands == commands && a.by == WAIVER_BY)
    {
        task.record_assumption(Assumption {
            question: format!("{} fails on the base too: waive it?", run.command),
            answer: WAIVED.to_string(),
            evidence: att.evidence,
            by: WAIVER_BY.to_string(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: task.attempts.last().map(|a| a.n),
            commands,
            overturned: false,
            owner_answer: None,
        });
    }
    BaseVerdict::Waived
}

/// Whether code saw the environment fail: the worktree setup did not finish
/// or the run timed out.
fn environmental(run: &VerifyOutcome) -> bool {
    run.setup_failed || is_interrupted(run)
}

/// The cause to store for an agent's `cause`: `environment` only with a
/// signal code checked, else none. The claim stays advice (the question and
/// the assumption carry it) and never restarts the task.
fn typed_cause(claimed: Blame, signal: bool) -> Option<Blame> {
    match claimed {
        Blame::Environment if !signal => None,
        other => Some(other),
    }
}

/// The typed cause on the task's last implement attempt.
fn set_cause(task: &mut Task, cause: Option<Blame>) {
    crate::task::attempts::record_cause(task, cause);
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
    comparison: Comparison<'_>,
    runner: CheckRunner<'_>,
) -> BaseVerdict {
    let Comparison {
        failed,
        base: base_run,
    } = comparison;
    match compare(failed, base_run) {
        Compared::Covered(tests) => {
            waive(task, failed, &tests, false);
            return BaseVerdict::Waived;
        }
        Compared::Unparsed if base_run.code != Some(0) => {
            return settle_unclear(app, task, comparison, runner).await
        }
        Compared::Unparsed => {}
        Compared::New(_) => {}
    }
    // New failures: one re-run tells a flaky failure from the task's own. It
    // is flaky only when the re-run passes or shows nothing the base lacks.
    let again = run_verify_commands(runner, std::slice::from_ref(&failed.command)).await;
    let Some(again) = again.first().filter(|r| !is_interrupted(r)) else {
        return BaseVerdict::NewFailures(Box::new(failed.clone()));
    };
    let tests = failed.failing.clone();
    if again.code == Some(0) {
        if base_run.code == Some(0) {
            waive_rerun(task, failed);
        } else {
            waive(task, failed, &tests, true);
        }
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

    #[test]
    fn an_environment_claim_is_stored_only_with_a_code_checked_signal() {
        let mut quiet = run(1, "boom");
        assert_eq!(typed_cause(Blame::Environment, environmental(&quiet)), None);
        quiet.setup_failed = true;
        assert_eq!(
            typed_cause(Blame::Environment, environmental(&quiet)),
            Some(Blame::Environment)
        );
        assert_eq!(
            typed_cause(Blame::Incomplete, false),
            Some(Blame::Incomplete)
        );
        assert_eq!(typed_cause(Blame::Inherited, false), Some(Blame::Inherited));
    }

    fn run(code: i32, output: &str) -> VerifyOutcome {
        let report = crate::policy::test_output::parse(output, "");
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
    fn a_ci_gate_named_only_in_a_criterion_check_is_still_waived_on_equal_names() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["Checks pass -- check: npm run ci".into()];
        // Not the task's target, so nothing keeps the gate from a waiver ...
        assert!(!crate::engine::verification::base::answers::is_task_target(
            &task,
            "npm run ci",
            &[]
        ));
        // ... and every failing name is in the base run.
        let base = run(1, BASE);
        assert!(matches!(
            compare(&run(1, BASE), &base),
            Compared::Covered(_)
        ));
    }
}
