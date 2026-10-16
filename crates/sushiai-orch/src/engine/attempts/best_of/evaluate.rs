use crate::engine::attempts::best_of::candidate::CandidateRun;
use crate::engine::state::App;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::baseline::gated_held_out;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::held_out::run_held_out;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::settings::ScopedCheck;
use crate::task::attempt::CheckTally;
use crate::task::attempt::VerifyOutcome;
use crate::task::state::Task;
use crate::verify::scoping::filter_scoped;

pub(super) struct Eval {
    /// It finished, changed files and every check exited 0.
    pub(super) passed: bool,
    /// Every plain verify command exited 0 (checks excluded).
    pub(super) verify_ok: bool,
    /// Failed verify commands and checks; `u32::MAX` when it never finished.
    pub(super) failing: u32,
    pub(super) verify: Vec<VerifyOutcome>,
    pub(super) tally: CheckTally,
}

pub(super) fn run_finished(res: &Result<harness::RunOutcome, RunError>) -> bool {
    matches!(res, Ok(o) if !o.stalled && !o.over_budget && !o.time_budget && o.looped.is_none())
}

/// A candidate's verify commands, then (with `groundedChecks`) the gated
/// checks and the held-out one; all of them run so the candidates can be
/// compared by how many fail.
pub(super) async fn evaluate(
    app: &App,
    task: &Task,
    run: &CandidateRun,
    scoped_checks: &[ScopedCheck],
    cancel: &CancelToken,
) -> Eval {
    let worktree = run.worktree.as_path();
    let base_sha = task.base_sha.as_str();
    let sandbox = app.task_sandbox(&task.repo, worktree, &run.run_dir);
    let sandbox = sandbox.as_ref();
    let finished = run_finished(&run.outcome);
    let (wt, own) = (worktree.to_path_buf(), task.own_diff());
    let changed = tokio::task::spawn_blocking(move || {
        let _ = crate::git::changes::stage_all(&wt);
        own.changed_files(&wt).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    if !finished || changed.is_empty() {
        return Eval {
            passed: false,
            verify_ok: false,
            failing: u32::MAX,
            verify: vec![],
            tally: CheckTally::default(),
        };
    }
    // No decision line: this runs once per side, and the winner's own
    // attempt records the skip.
    let (verify_commands, _) = filter_scoped(scoped_checks, &task.repo, &task.verify, &changed);
    let mut verify = run_verify_commands(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(app, Some(&task.id)),
            worktree,
            sandbox,
            base_sha,
            cancel,
        ),
        &verify_commands,
    )
    .await;
    let plain = verify.len();

    let commands: Vec<String> = gated_checks(task).into_iter().map(|c| c.run).collect();
    if !commands.is_empty() {
        verify.extend(
            run_verify_commands(
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(app, Some(&task.id)),
                    worktree,
                    sandbox,
                    base_sha,
                    cancel,
                ),
                &commands,
            )
            .await,
        );
    }
    if let Some(held) = gated_held_out(task) {
        let (result, _) = run_held_out(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                worktree,
                sandbox,
                &(task).base_sha,
                cancel,
            ),
            task,
            &held,
        )
        .await;
        verify.push(result);
    }

    let failed = |v: &[VerifyOutcome]| v.iter().filter(|o| o.code != Some(0)).count() as u32;
    let tally = CheckTally {
        passed: (verify.len() - plain) as u32 - failed(&verify[plain..]),
        failed: failed(&verify[plain..]),
    };
    let failing = failed(&verify);
    Eval {
        passed: failing == 0,
        verify_ok: failed(&verify[..plain]) == 0,
        failing,
        verify,
        tally,
    }
}

/// A candidate whose protected files had to be restored cannot have passed:
/// its checks ran on the originals, not on what it wrote.
pub(super) fn disqualify_restored(
    eval: &mut Eval,
    restored: &[String],
    label: &str,
    decisions: &mut Vec<String>,
) {
    if restored.is_empty() {
        return;
    }
    eval.passed = false;
    eval.failing = eval.failing.max(1);
    decisions.push(format!(
        "Best-of: candidate {label} changed protected files ({}); they were put back and it does not count as passed",
        restored.join(", ")
    ));
}
