use crate::delivery::pr::checkout::TempWorktree;
use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::engine::verification::scoping::split_deferred;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::FailureKind;
use crate::task::state::Task;
use std::path::Path;
use std::sync::Arc;

/// Carries the done task's work onto the newest `origin/<base>` before it is
/// pushed, with the task's checks run again when the base moved, as landing
/// does. A branch on an old base would show newer base files (a workflow, a
/// signature) as the task's own change. The branch moves only when the carry
/// and the checks pass; the decisions are saved either way. A carry that
/// conflicts is not a dead end: see [`resolve_conflict`].
pub(super) struct CarriedWork {
    pub(super) work: Task,
    pub(super) before: usize,
    pub(super) committed: Result<String, String>,
    pub(super) conflicted: bool,
    pub(super) contained: bool,
}
pub(super) async fn carry_work(
    app: &Arc<App>,
    task: &Task,
    temp: &TempWorktree,
    run_dir: &Path,
    upstream: &str,
) -> Result<CarriedWork, String> {
    let finals = {
        let settings = app.settings.read().unwrap();
        split_deferred(app, &settings, task).0
    };
    let mut work = task.clone();
    let before = work.decisions.len();
    let outcome = crate::engine::graph::carry::CarryRun::new(
        app,
        &mut work,
        None,
        run_dir,
        &CancelToken::new(),
    )
    .in_worktree(&temp.path)
    .onto(upstream, "Another task", &finals)
    .await;
    let conflicted = matches!(
        &outcome,
        Err(Landing::Failed {
            kind: FailureKind::Conflict,
            ..
        })
    );
    let contained = matches!(outcome, Ok(true));
    let committed = match outcome {
        Ok(false) => {
            let (wt, title, tid, n) = (
                temp.path.clone(),
                task.title.clone(),
                task.id.clone(),
                task.attempts.len().max(1) as u32,
            );
            tokio::task::spawn_blocking(move || {
                crate::delivery::git::commit::commit(&wt, &title, &tid, n)?;
                crate::git::history::head_sha(&wt)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("could not commit the carried work: {e}"))
        }
        Ok(true) => Err(format!("{upstream} already contains this work")),
        Err(Landing::Failed { detail, .. }) => Err(detail),
        Err(_) => Err("the carry was interrupted".to_string()),
    };
    Ok(CarriedWork {
        work,
        before,
        committed,
        conflicted,
        contained,
    })
}
