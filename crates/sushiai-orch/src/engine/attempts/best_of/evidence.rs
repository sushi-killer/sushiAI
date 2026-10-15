use crate::engine::attempts::best_of::evaluate::disqualify_restored;
use crate::engine::attempts::best_of::evaluate::evaluate;
use crate::engine::attempts::best_of::evaluate::run_finished;
use crate::engine::attempts::best_of::evaluate::Eval;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::protected::guard_protected;
use crate::engine::state::App;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::settings::Settings;
use crate::task::Task;
use std::path::Path;

pub(super) struct Evaluated {
    pub(super) a: Eval,
    pub(super) b: Eval,
    pub(super) restored_a: Vec<String>,
    pub(super) restored_b: Vec<String>,
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate_pair(
    app: &App,
    task: &Task,
    worktree: &Path,
    base_sha: &str,
    run_dir: &Path,
    sibling: &Sibling,
    a_res: &Result<harness::RunOutcome, RunError>,
    b_res: &Result<harness::RunOutcome, RunError>,
    settings: &Settings,
    cancel: &CancelToken,
    decisions: &mut Vec<String>,
) -> Evaluated {
    let own = task.own_diff();
    // A candidate that touched a protected file has it put back before it is
    // judged, and does not count as passed for the pick.
    let (restored_a, restored_b) = tokio::join!(
        guard_protected(task, worktree, &[]),
        guard_protected(task, &sibling.worktree, &[])
    );
    let (restored_a, restored_b) = (
        restored_a.unwrap_or_default(),
        restored_b.unwrap_or_default(),
    );
    let scoped = scoped_checks_for(settings, task);
    let sandbox_a = app.task_sandbox(&task.repo, worktree, run_dir);
    let sandbox_b = app.task_sandbox(&task.repo, &sibling.worktree, &sibling.run_dir);
    let (mut eval_a, mut eval_b) = tokio::join!(
        evaluate(
            app,
            task,
            worktree,
            base_sha,
            &own,
            run_dir,
            run_finished(a_res),
            sandbox_a.as_ref(),
            &scoped,
            cancel
        ),
        evaluate(
            app,
            task,
            &sibling.worktree,
            base_sha,
            &own,
            &sibling.run_dir,
            run_finished(b_res),
            sandbox_b.as_ref(),
            &scoped,
            cancel
        ),
    );

    disqualify_restored(&mut eval_a, &restored_a, "a", decisions);
    disqualify_restored(&mut eval_b, &restored_b, "b", decisions);
    decisions.extend(
        eval_a
            .verify
            .iter()
            .chain(&eval_b.verify)
            .flat_map(|r| r.decisions.iter().cloned()),
    );

    Evaluated {
        a: eval_a,
        b: eval_b,
        restored_a,
        restored_b,
    }
}
