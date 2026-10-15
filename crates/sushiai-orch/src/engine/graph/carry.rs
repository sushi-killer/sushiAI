use crate::engine::graph::landing::{
    carry_changed_reviewed, Landing, MAX_REREVIEWS, REREVIEW_LINE,
};
use crate::engine::graph::relay::conflict_detail;
use crate::engine::protected::adopt_carried_tests;
use crate::engine::protected::carry_with_planner_tests;
use crate::engine::reports::record_landing;
use crate::engine::state::App;
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::engine::verification::scoping::scope_task_checks;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::FailureKind;
use crate::task::Task;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// One carry and delivery run, including the checkout it operates on and its receipts.
pub(in crate::engine) struct CarryRun<'a> {
    app: &'a Arc<App>,
    task: &'a mut Task,
    index: Option<usize>,
    run_dir: &'a Path,
    cancel: &'a CancelToken,
    worktree: PathBuf,
}
impl<'a> CarryRun<'a> {
    pub(in crate::engine) fn new(
        app: &'a Arc<App>,
        task: &'a mut Task,
        index: Option<usize>,
        run_dir: &'a Path,
        cancel: &'a CancelToken,
    ) -> Self {
        let worktree = PathBuf::from(&task.worktree);
        Self {
            app,
            task,
            index,
            run_dir,
            cancel,
            worktree,
        }
    }
    pub(in crate::engine) fn in_worktree(mut self, worktree: &Path) -> Self {
        self.worktree = worktree.to_path_buf();
        self
    }
    pub(in crate::engine) async fn onto(
        &mut self,
        branch: &str,
        first: &str,
        extra_final: &[String],
    ) -> Result<bool, Landing> {
        let failed = |kind, detail| Err(Landing::Failed { kind, detail });
        let branch = branch.to_string();
        let carried = self.carry(&branch).await;
        match carried {
            Ok(crate::delivery::git::carry::Rebase::Unchanged) => {}
            Ok(crate::delivery::git::carry::Rebase::Moved { new_sha, rewritten }) => {
                self.task.record_decision(if rewritten {
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
                return self
                    .check_carried(&branch, first, &new_sha, extra_final)
                    .await;
            }
            Ok(crate::delivery::git::carry::Rebase::Conflicts {
                new_sha,
                files,
                rewritten,
            }) => {
                if rewritten {
                    self.task.record_decision(format!(
                    "Land: {branch} was rewritten; carried the work onto it at {} with conflicts in {}",
                    short_sha(&new_sha),
                    files.join(", ")
                ));
                }
                crate::task::transitions::move_base(self.task, new_sha.clone());
                return failed(
                    FailureKind::Conflict,
                    conflict_detail(&branch, &new_sha, &files, &self.task.id),
                );
            }
            Ok(crate::delivery::git::carry::Rebase::Skipped { reason }) => {
                return failed(
                    FailureKind::Error,
                    format!("Could not carry the work onto {branch} ({reason})."),
                );
            }
            Err(e) => {
                return failed(
                    FailureKind::Error,
                    format!("Could not carry the work onto {branch} ({e})."),
                );
            }
        }
        Ok(false)
    }
    async fn carry(
        &mut self,
        branch: &str,
    ) -> Result<crate::delivery::git::carry::Rebase, crate::git::GitError> {
        let task = &mut *self.task;
        let worktree = self.worktree.as_path();
        let (wt, mut snapshot, b) = (worktree.to_path_buf(), task.clone(), branch.to_string());
        let carried = tokio::task::spawn_blocking(move || {
            // Commits the agent made itself go back to being uncommitted work,
            // so they are carried (and later committed) like the rest.
            if crate::git::history::head_sha(&wt)? != snapshot.base_sha {
                crate::delivery::git::carry::uncommit_to(&wt, &snapshot.base_sha)?;
            }
            let carried = carry_with_planner_tests(&mut snapshot, &wt, &b)?;
            Ok((carried, snapshot.planner_tests.clone()))
        })
        .await
        .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
        carried.map(|(carried, tests)| {
            adopt_carried_tests(task, tests, &carried);
            carried
        })
    }
    async fn check_carried(
        &mut self,
        branch: &str,
        first: &str,
        new_sha: &str,
        extra_final: &[String],
    ) -> Result<bool, Landing> {
        let task = &mut *self.task;
        let idx = self.index;
        let worktree = self.worktree.as_path();
        crate::task::transitions::move_base(task, new_sha.to_string());
        let (wt, b) = (worktree.to_path_buf(), new_sha.to_string());
        let changed = tokio::task::spawn_blocking(move || {
            crate::git::changes::changed_files(&wt, &b).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if let Some(idx) = idx {
            crate::task::attempts::record_changed_files(task, idx, changed.clone());
        }
        if changed.is_empty() {
            task.record_decision(format!("Land: {branch} already contains this work"));
            return Ok(true);
        }
        // A carry that rewrote a file the review saw leaves that review
        // judging another tree: verify and the review run again.
        if let Some(idx) = idx {
            let files = carry_changed_reviewed(worktree, &task.attempts[idx], new_sha).await;
            if !files.is_empty() && task.loop_state.rereviews < MAX_REREVIEWS {
                task.count_rereview();
                task.record_decision(format!(
                        "{REREVIEW_LINE} onto {branch} changed {} which the review saw; verify and the review run again on the carried tree",
                        files.join(", ")
                    ));
                return Err(Landing::Rereview);
            }
        }
        self.verify_carried(branch, first).await?;
        self.verify_finals(branch, first, extra_final).await?;
        Ok(false)
    }
    async fn verify_carried(&mut self, branch: &str, first: &str) -> Result<(), Landing> {
        let app = self.app;
        let task = &mut *self.task;
        let idx = self.index;
        let worktree = self.worktree.as_path();
        let run_dir = self.run_dir;
        let cancel = self.cancel;
        let failed = |kind, detail| Err(Landing::Failed { kind, detail });
        let (verify_commands, skipped) =
            scope_task_checks(app, task, worktree, &task.base_sha, &task.verify).await;
        record_check_decisions(task, skipped);
        let (results, cache_line) = run_verify_cached(
            app,
            &task.id,
            &task.repo,
            worktree,
            run_dir,
            &task.base_sha,
            &verify_commands,
            cancel,
        )
        .await;
        record_check_decisions(task, cache_line);
        if cancel.is_cancelled() {
            return Err(Landing::Cancelled);
        }
        record_check_outcomes(app, task, &results);
        if let Some(idx) = idx {
            crate::task::attempts::record_verify(
                task,
                idx,
                crate::task::attempts::VerifyObservation::Replaced(results.clone()),
            );
        }
        if let Some(v) = results.iter().find(|v| v.code != Some(0)) {
            return failed(
                FailureKind::Verify,
                format!(
                    "{first} landed on {branch} first; carried onto it, {} exited {}.\n{}",
                    v.command,
                    v.code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                    v.tail
                ),
            );
        }
        Ok(())
    }
    async fn verify_finals(
        &mut self,
        branch: &str,
        first: &str,
        extra_final: &[String],
    ) -> Result<(), Landing> {
        let app = self.app;
        let task = &mut *self.task;
        let idx = self.index;
        let worktree = self.worktree.as_path();
        let run_dir = self.run_dir;
        let cancel = self.cancel;
        let failed = |kind, detail| Err(Landing::Failed { kind, detail });
        let (extra_final, skipped) =
            scope_task_checks(app, task, worktree, &task.base_sha, extra_final).await;
        record_check_decisions(task, skipped);
        if !extra_final.is_empty() {
            let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
            let finals = run_verify_commands(
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
                    worktree,
                    sandbox.as_ref(),
                    &task.base_sha,
                    cancel,
                ),
                &extra_final,
            )
            .await;
            record_check_outcomes(app, task, &finals);
            if cancel.is_cancelled() {
                return Err(Landing::Cancelled);
            }
            if let Some(idx) = idx {
                crate::task::attempts::record_verify(
                    task,
                    idx,
                    crate::task::attempts::VerifyObservation::Appended(finals.clone()),
                );
            }
            if let Some(v) = finals.iter().find(|v| v.code != Some(0)) {
                return failed(
                        FailureKind::Verify,
                        format!(
                            "{first} landed on {branch} first; carried onto it, final check {} exited {}.\n{}",
                            v.command,
                            v.code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "null".to_string()),
                            v.tail
                        ),
                    );
            }
        }
        Ok(())
    }
    pub(in crate::engine) async fn land_on_parent(&mut self, parent_id: &str) -> Landing {
        let app = self.app;
        let cancel = self.cancel;
        // A link of a relay that a later sibling continues commits on its own
        // branch; the last link lands the whole chain on the parent.
        let next = app
            .repo_tasks(&self.task.repo)
            .into_iter()
            .find(|t| t.queue.relay_of.as_deref() == Some(self.task.id.as_str()) && !t.archived);
        if let Some(next) = next {
            return self.commit_relay(&next).await;
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

        if let Err(landing) = self.gather_relay(&branch).await {
            return landing;
        }

        match self.onto(&branch, "Another subtask", &[]).await {
            Ok(true) => {
                record_landing(self.task).await;
                return Landing::Landed;
            }
            Ok(false) => {}
            Err(landing) => return landing,
        }
        self.commit_on_parent(&parent).await
    }
    async fn commit_relay(&mut self, next: &Task) -> Landing {
        let task = &mut *self.task;
        let idx = self.index;
        let attempt_n = idx.map_or(0, |idx| task.attempts[idx].n);
        let worktree = self.worktree.as_path();

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
    async fn gather_relay(&mut self, branch: &str) -> Result<(), Landing> {
        let task = &mut *self.task;
        let worktree = self.worktree.as_path();
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
    async fn commit_on_parent(&mut self, parent: &Task) -> Landing {
        let task = &mut *self.task;
        let idx = self.index;
        let attempt_n = idx.map_or(0, |idx| task.attempts[idx].n);
        let worktree = self.worktree.as_path();
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
            if let Err(e) = crate::delivery::git::carry::fast_forward(&parent_wt, &sha) {
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
