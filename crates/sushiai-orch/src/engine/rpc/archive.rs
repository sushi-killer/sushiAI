use crate::api::validation::validate_task_id;
use crate::engine::state::App;
use crate::task::TaskStatus;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

impl App {
    /// Cancels the loop and waits for it to actually exit before touching
    /// the filesystem, so a still-running attempt can never write into a
    /// directory that's mid-deletion.
    pub(in crate::engine) async fn handle_task_delete(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            /// Set by the MCP bridge; absent for the owner UI and daemon cleanup.
            #[serde(default)]
            source: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        if p.source.is_some() {
            // An agent deletes only what is already stopped: ending a live
            // task is task_stop's job.
            if let Some(task) = self.store.load_task(&p.id).ok().flatten() {
                if matches!(
                    task.status,
                    TaskStatus::Running
                        | TaskStatus::Drafting
                        | TaskStatus::Waiting
                        | TaskStatus::Landing
                ) {
                    return Err("cannot delete a running, drafting, waiting or landing task; use task_stop first".to_string());
                }
            }
            if self.controls.lock().unwrap().contains_key(&p.id) {
                return Err(
                    "cannot delete a task with a live loop; use task_stop first".to_string()
                );
            }
        }
        let removed = self.controls.lock().unwrap().remove(&p.id);
        if let Some(ctrl) = removed {
            ctrl.signals.cancel.cancel();
            let _ = ctrl.handle.await;
        }
        let doomed = self.store.load_task(&p.id).ok().flatten();
        let repo = doomed.as_ref().map(|t| t.repo.clone());
        if let Some(t) = doomed {
            let _ = tokio::task::spawn_blocking(move || {
                crate::delivery::git::recovery::remove_task_worktree(
                    Path::new(&t.repo.clone()),
                    Path::new(&t.worktree),
                );
                crate::delivery::git::recovery::delete_branch(
                    Path::new(&t.repo.clone()),
                    &t.branch,
                    &t.id,
                );
            })
            .await;
        }
        let dir = self.store.task_dir(&p.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        // A deleted dependency no longer holds anything back.
        if let Some(repo) = repo {
            self.advance_graph(&repo);
        }
        Ok(json!({}))
    }
}

impl App {
    /// Hides a task from the default `task.list` without touching its
    /// worktree, branch, or record -- refused while a loop could still be
    /// mutating it (running, drafting a plan, or parked waiting for an
    /// answer), same as `task.delete` would need to stop it first, except
    /// this is never destructive so there's nothing to reconcile after. Also
    /// refused while a loop is merely queued behind the concurrency limit
    /// (present in `controls` even though `status` still reads `Queued`) --
    /// that loop is still going to run and mutate the task the moment a
    /// slot frees up, same hazard as an already-running one.
    pub(in crate::engine) async fn handle_task_archive(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if matches!(
            task.status,
            TaskStatus::Running | TaskStatus::Drafting | TaskStatus::Waiting | TaskStatus::Landing
        ) {
            return Err("cannot archive a running, drafting, waiting or landing task; use task_stop to end it first"
                    .to_string());
        }
        if self.controls.lock().unwrap().contains_key(&p.id) {
            return Err(
                "cannot archive a task with a live loop; use task_stop to end it first".to_string(),
            );
        }
        let decisions_before = task.decisions.len();
        self.release_worktree(&mut task, "archived").await;
        let task = self.update_task(&p.id, |fresh| {
            fresh.archive(task.worktree_removed, task.deliverables.clone());
            fresh.record_decisions(task.decisions[decisions_before..].iter().cloned());
        })?;
        // An archived child no longer holds its parent back.
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

impl App {
    pub(in crate::engine) async fn handle_task_unarchive(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let decisions_before = task.decisions.len();
        if matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed) {
            if let Err(e) = self.ensure_worktree(&mut task).await {
                task.record_decision(format!("Worktree: {e}"));
            }
        }
        let task = self.update_task(&p.id, |fresh| {
            fresh.unarchive(task.worktree_removed);
            fresh.record_decisions(task.decisions[decisions_before..].iter().cloned());
        })?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}
