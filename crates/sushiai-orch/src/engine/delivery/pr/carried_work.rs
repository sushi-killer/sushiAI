use crate::delivery::pr::checkout::TempWorktree;
use crate::engine::graph::carry::{CarryFailure, CarryTree};
use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::engine::verification::scoping::split_deferred;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::FailureKind;
use crate::task::state::Task;
use crate::task::VerifyOutcome;
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
    pub(super) failed_check: Option<VerifyOutcome>,
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
    let cancel = CancelToken::new();
    let tree = CarryTree::new(&temp.path, run_dir, None, &cancel);
    let mut outcome = tree.onto(app, &mut work, upstream, &finals).await;
    // An already-carried branch can owe a new check after an Owner revokes a waiver.
    if matches!(outcome, Ok(false)) && task.base_sha == work.base_sha {
        outcome = tree.check_carried(app, &mut work, upstream, &finals).await;
    }
    let conflicted = matches!(
        &outcome,
        Err(CarryFailure::Delivery(Landing::Failed {
            kind: FailureKind::Conflict,
            ..
        }))
    );
    let failed_check = match &outcome {
        Err(CarryFailure::Check(failed)) => Some((**failed).clone()),
        _ => None,
    };
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
        Err(CarryFailure::Check(ref failed)) => Err(format!(
            "post-carry check {} failed: {}",
            failed.command, failed.tail
        )),
        Err(CarryFailure::Delivery(Landing::Failed { detail, .. })) => Err(detail),
        Err(_) => Err("the carry was interrupted".to_string()),
    };
    Ok(CarriedWork {
        work,
        before,
        committed,
        conflicted,
        contained,
        failed_check,
    })
}
