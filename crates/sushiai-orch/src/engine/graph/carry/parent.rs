use super::CarryTree;
use crate::engine::graph::landing::Landing;
use crate::engine::reports::record_landing;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::task::{FailureKind, Task};
use std::path::PathBuf;
use std::sync::Arc;

impl CarryTree<'_> {
    pub(in crate::engine) async fn land_on_parent(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        parent_id: &str,
    ) -> Landing {
        let cancel = self.cancel;
        // A link of a relay that a later sibling continues commits on its own
        // branch; the last link lands the whole chain on the parent.
        let next = app
            .repo_tasks(&task.repo)
            .into_iter()
            .find(|t| t.queue.relay_of.as_deref() == Some(task.id.as_str()) && !t.archived);
        if let Some(next) = next {
            return self.commit_relay(task, &next).await;
        }
        let lock = app.landing_lock(parent_id);
        let _landing = tokio::select! {
            _ = cancel.cancelled() => return Landing::Cancelled,
            guard = lock.lock_owned() => guard,
        };
        let Ok(Some(parent)) = app.store.load_task(parent_id) else {
            return Landing::NoParent;
        };
        let branch = parent.branch.clone();

        if let Err(landing) = self.gather_relay(task, &branch).await {
            return landing;
        }

        match self.onto(app, task, &branch, &[]).await {
            Ok(true) => {
                record_landing(task).await;
                return Landing::Landed;
            }
            Ok(false) => {}
            Err(failure) => return failure.into_landing(),
        }
        self.commit_on_parent(task, &parent).await
    }
    async fn commit_relay(&self, task: &mut Task, next: &Task) -> Landing {
        let idx = self.index;
        let attempt_n = idx.map_or(0, |idx| task.attempts[idx].n);
        let worktree = self.worktree;

        let (wt, title, tid) = (worktree.to_path_buf(), task.title.clone(), task.id.clone());
        let committed = tokio::task::spawn_blocking(move || {
            crate::delivery::git::commit::commit(&wt, &title, &tid, attempt_n)
        })
        .await
        .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
        match committed {
            Ok(()) => {
                task.record_decision(format!(
                    "Land: committed on {} for \"{}\" to continue from",
                    task.branch, next.title
                ));
                record_landing(task).await;
                Landing::Landed
            }
            Err(e) => Landing::Failed {
                kind: FailureKind::Error,
                detail: format!("Could not commit for the relay: {e}"),
            },
        }
    }
    async fn gather_relay(&self, task: &mut Task, branch: &str) -> Result<(), Landing> {
        let worktree = self.worktree;
        let failed = |kind, detail| Err(Landing::Failed { kind, detail });
        // The last link of a relay: everything since the chain began is this
        // task's work now, carried onto the parent like any other.
        if let Some(chain_base) = task.queue.relay_base.clone() {
            let (wt, base) = (worktree.to_path_buf(), chain_base.clone());
            let reset = tokio::task::spawn_blocking(move || {
                crate::delivery::git::carry::uncommit_to(&wt, &base)
            })
            .await
            .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
            if let Err(e) = reset {
                return failed(
                    FailureKind::Error,
                    format!("Could not gather the relay's work: {e}"),
                );
            }
            task.record_decision(format!(
                "Land: lands the relay's chain from {} onto {branch}",
                short_sha(&chain_base)
            ));
            crate::task::transitions::move_base(task, chain_base);
            task.record_base_ref(Some(branch.to_string()));
            task.record_relay_base(None);
        }
        Ok(())
    }
    async fn commit_on_parent(&self, task: &mut Task, parent: &Task) -> Landing {
        let idx = self.index;
        let attempt_n = idx.map_or(0, |idx| task.attempts[idx].n);
        let worktree = self.worktree;
        let branch = parent.branch.clone();
        let failed = |kind, detail| Landing::Failed { kind, detail };
        let (wt, parent_wt, title, tid, base) = (
            worktree.to_path_buf(),
            PathBuf::from(&parent.worktree),
            task.title.clone(),
            task.id.clone(),
            task.base_sha.clone(),
        );
        let landed = tokio::task::spawn_blocking(move || {
            crate::delivery::git::commit::commit(&wt, &title, &tid, attempt_n)?;
            let sha = crate::git::history::head_sha(&wt)?;
            if let Err(e) = crate::git::branches::fast_forward(&parent_wt, &sha) {
                // Keep the work, uncommitted, for the next attempt to carry.
                let _ = crate::delivery::git::carry::uncommit_to(&wt, &base);
                return Err(e);
            }
            Ok(sha)
        })
        .await
        .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
        match landed {
            Ok(sha) => {
                task.record_decision(format!("Land: landed on {branch} at {}", short_sha(&sha)));
                record_landing(task).await;
                Landing::Landed
            }
            Err(e) => failed(
                FailureKind::Error,
                format!("Could not land on {branch}: {e}"),
            ),
        }
    }
}
