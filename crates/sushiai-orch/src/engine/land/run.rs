use super::{commit, target, waiting};
use crate::engine::graph::carry::CarryTree;
use crate::engine::graph::landing::Landing;
use crate::engine::reports::record_landing;
use crate::engine::state::App;
use crate::engine::verification::scoping::split_deferred;
use crate::git::{branches::BranchMove, paths::short_sha};
use crate::task::{FailureKind, Task};
use std::sync::Arc;

const MAX_RECARRIES: u32 = 3;

/// Carries, checks and squashes the work before moving the admitted base branch.
pub(in crate::engine) async fn land_on_base(
    app: &Arc<App>,
    task: &mut Task,
    tree: &CarryTree<'_>,
) -> Landing {
    let (land_on_default, after_land) = {
        let settings = app.settings.read().unwrap();
        (
            settings.may_land_on_default(&task.repo),
            settings.after_land.clone(),
        )
    };
    let target = match target::resolve(app, task, land_on_default).await {
        Ok(target) => target,
        Err(landing) => return landing,
    };
    let branch = &target.branch;
    let lock = app.landing_lock(&format!("base\0{}\0{branch}", task.repo));
    let _landing = tokio::select! {
        _ = tree.cancel.cancelled() => return Landing::Cancelled,
        guard = lock.lock_owned() => guard,
    };
    let finals = {
        let settings = app.settings.read().unwrap();
        split_deferred(app, &settings, task).0
    };
    let mut recarries = 0;
    loop {
        match tree.onto(app, task, branch, &finals).await {
            Ok(true) => return commit::contained(app, task, &target).await,
            Ok(false) => {}
            Err(failure) => return failure.into_landing(),
        }
        let moved = commit::move_branch(tree, task, branch).await;
        if !matches!(moved, Ok((_, BranchMove::Dirty { .. }))) {
            task.end_dirty_wait();
        }
        match moved {
            Ok((sha, BranchMove::Done { checkout })) => {
                task.record_landed(Some(sha.clone()));
                task.record_unlanded(None);
                task.record_decision(format!("Land: landed on {branch} at {}", short_sha(&sha)));
                commit::redirected(app, task, branch, &sha, &target.done_tasks).await;
                record_landing(task).await;
                commit::after_land(task, &checkout, &after_land).await;
                return Landing::Landed;
            }
            Ok((_, BranchMove::Dirty { checkout })) => return waiting::dirty(app, task, branch, &checkout),
            Ok((_, BranchMove::Diverged)) if recarries < MAX_RECARRIES => {
                recarries += 1;
                task.record_decision(format!("Land: {branch} moved while landing; carrying the work onto it again (try {recarries} of {MAX_RECARRIES})"));
            }
            Ok((_, BranchMove::Diverged)) => return Landing::Failed {
                kind: FailureKind::Error,
                detail: format!("{branch} moved while landing, {MAX_RECARRIES} times in a row; carry the work onto it again."),
            },
            Err(error) => return Landing::Failed {kind:FailureKind::Error, detail:format!("Could not land on {branch}: {error}")},
        }
    }
}
