use crate::engine::graph::landing::{
    carry_changed_reviewed, Landing, MAX_REREVIEWS, REREVIEW_LINE,
};
use crate::engine::graph::relay::conflict_detail;
use crate::engine::protected::carry::{adopt_carried_tests, carry_with_planner_tests};
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::{FailureKind, Task, VerifyOutcome};
use std::path::Path;
use std::sync::Arc;

mod checks;
mod parent;

/// The checkout and run files used by a carry, with its optional attempt receipt.
pub(in crate::engine) struct CarryTree<'a> {
    pub(in crate::engine) worktree: &'a Path,
    pub(in crate::engine) run_dir: &'a Path,
    pub(in crate::engine) index: Option<usize>,
    pub(in crate::engine) cancel: &'a CancelToken,
}

impl<'a> CarryTree<'a> {
    pub(in crate::engine) fn new(
        worktree: &'a Path,
        run_dir: &'a Path,
        index: Option<usize>,
        cancel: &'a CancelToken,
    ) -> Self {
        Self {
            worktree,
            run_dir,
            index,
            cancel,
        }
    }

    pub(in crate::engine) async fn onto(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        branch: &str,
        extra_final: &[String],
    ) -> Result<bool, CarryFailure> {
        use crate::delivery::git::carry::Rebase;
        match self.carry(task, branch).await {
            Ok(Rebase::Unchanged) => Ok(false),
            Ok(Rebase::Moved { new_sha, rewritten }) => {
                task.record_decision(if rewritten {
                    format!(
                        "Land: {branch} was rewritten; carried the work onto it at {}",
                        short_sha(&new_sha)
                    )
                } else {
                    format!(
                        "Land: carried the work onto {branch} at {}",
                        short_sha(&new_sha)
                    )
                });
                crate::task::transitions::move_base(task, new_sha);
                self.check_carried(app, task, branch, extra_final).await
            }
            Ok(Rebase::Conflicts {
                new_sha,
                files,
                rewritten,
            }) => {
                if rewritten {
                    task.record_decision(format!(
                        "Land: {branch} was rewritten; carried the work onto it at {} with conflicts in {}",
                        short_sha(&new_sha), files.join(", ")
                    ));
                }
                crate::task::transitions::move_base(task, new_sha.clone());
                let detail = conflict_detail(branch, &new_sha, &files);
                task.record_carry_conflicts(files);
                Err(Landing::Failed {
                    kind: FailureKind::Conflict,
                    detail,
                }
                .into())
            }
            Ok(Rebase::Skipped { reason }) => Err(carry_error(branch, reason).into()),
            Err(error) => Err(carry_error(branch, error.to_string()).into()),
        }
    }

    async fn carry(
        &self,
        task: &mut Task,
        branch: &str,
    ) -> Result<crate::delivery::git::carry::Rebase, crate::git::GitError> {
        let (wt, mut snapshot, branch) = (
            self.worktree.to_path_buf(),
            task.clone(),
            branch.to_string(),
        );
        let carried = tokio::task::spawn_blocking(move || {
            if crate::git::history::head_sha(&wt)? != snapshot.base_sha {
                crate::delivery::git::carry::uncommit_to(&wt, &snapshot.base_sha)?;
            }
            let carried = carry_with_planner_tests(&mut snapshot, &wt, &branch)?;
            Ok((carried, snapshot.planner_tests.clone()))
        })
        .await
        .unwrap_or_else(|error| Err(crate::git::GitError(error.to_string())));
        carried.map(|(carried, tests)| {
            adopt_carried_tests(task, tests, &carried);
            carried
        })
    }

    pub(in crate::engine) async fn check_carried(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        branch: &str,
        extra_final: &[String],
    ) -> Result<bool, CarryFailure> {
        let (wt, base) = (self.worktree.to_path_buf(), task.base_sha.clone());
        let changed = tokio::task::spawn_blocking(move || {
            crate::git::changes::changed_files(&wt, &base).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if let Some(index) = self.index {
            crate::task::attempts::record_changed_files(task, index, changed.clone());
        }
        if changed.is_empty() {
            task.record_decision(format!("Land: {branch} already contains this work"));
            return Ok(true);
        }
        if let Some(index) = self.index {
            let files =
                carry_changed_reviewed(self.worktree, &task.attempts[index], &task.base_sha).await;
            if !files.is_empty() && task.loop_state.rereviews < MAX_REREVIEWS {
                task.count_rereview();
                task.record_decision(format!(
                    "{REREVIEW_LINE} onto {branch} changed {} which the review saw; verify and the review run again on the carried tree", files.join(", ")
                ));
                return Err(Landing::Rereview.into());
            }
        }
        self.verify(app, task, &task.verify.clone(), false).await?;
        self.verify(app, task, extra_final, true).await?;
        Ok(false)
    }
}

fn carry_error(branch: &str, reason: String) -> Landing {
    Landing::Failed {
        kind: FailureKind::Error,
        detail: format!("Could not carry the work onto {branch} ({reason})."),
    }
}

/// A measured check failure stays typed until the delivery caller chooses its next action.
pub(in crate::engine) enum CarryFailure {
    Check(Box<VerifyOutcome>),
    Delivery(Landing),
}
impl CarryFailure {
    pub(in crate::engine) fn into_landing(self) -> Landing {
        match self {
            Self::Check(failed) => checks::check_failure(&failed),
            Self::Delivery(landing) => landing,
        }
    }
}
impl From<Landing> for CarryFailure {
    fn from(landing: Landing) -> Self {
        Self::Delivery(landing)
    }
}

#[cfg(test)]
mod tests;
