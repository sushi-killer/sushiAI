use crate::api::validation::validate_task_id;
use crate::clip::truncate_chars;
use crate::engine::decision::implement_attempt_count;
use crate::engine::state::App;
use crate::task::graph::is_parent;
use crate::task::queue::operations::Starter;
use crate::task::AnswerSource;
use crate::task::TaskStatus;
use serde::Deserialize;
use std::path::PathBuf;

impl App {
    pub(in crate::engine) async fn handle_task_start(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            /// Set by the bridge for an agent caller (overseer, chat, MCP);
            /// absent for the owner.
            #[serde(default)]
            source: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        // An agent restarts a failed task once; the second time is the
        // owner's, or the ladder would loop through the overseer.
        if p.source.is_some() {
            self.agent_restart_gate(std::slice::from_ref(&p.id))?;
        }
        let already_running = self
            .controls
            .lock()
            .unwrap()
            .get(&p.id)
            .is_some_and(|c| !c.signals.cancel.is_cancelled());
        self.update_task_checked(&p.id, |task| {
            if task.archived {
                return Err("task is archived; unarchive it first".to_string());
            }
            task.clear_limit_pause();
            Ok(())
        })?;
        if already_running {
            if let Some(control) = self.controls.lock().unwrap().get(&p.id) {
                control.signals.resume.notify_one();
            }
        }
        if !already_running {
            let task = self
                .store
                .load_task(&p.id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "task not found".to_string())?;
            if task.archived {
                return Err("task is archived; unarchive it first".to_string());
            }
            if matches!(
                task.status,
                TaskStatus::Queued
                    | TaskStatus::Stopped
                    | TaskStatus::Failed
                    | TaskStatus::Drafting
                    | TaskStatus::Landing
            ) || (is_parent(&task, &self.repo_tasks(&task.repo))
                && matches!(task.status, TaskStatus::Running | TaskStatus::Waiting))
            {
                // A manual `task.start` on a still-drafting task (e.g. one
                // left there by a daemon that died mid-plan) means proceed
                // straight to implementing once planning finishes.
                let _ = self.update_task(&p.id, |t| t.clear_pr_conflict());
                self.start_task(&p.id, Starter::Owner);
            }
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }
}

impl App {
    /// `task.land {id}`: lands a done, unlanded top-level task now, through
    /// the same landing queue a `variant.land` task uses. The task goes back
    /// to `landing` and its loop does the rest; the default branch is still
    /// refused unless the repo is allowed in `settings.landOnDefaultRepos` (or `settings.landOnDefault`).
    pub(in crate::engine) async fn handle_task_land(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Done {
            return Err("only a done task can be landed".to_string());
        }
        if task.parent.is_some() {
            return Err("a subtask lands on its parent by itself".to_string());
        }
        if task.landed_sha.is_some() {
            return Err("task is already landed".to_string());
        }
        if task.archived {
            return Err("task is archived; unarchive it first".to_string());
        }
        if self.controls.lock().unwrap().contains_key(&p.id) {
            return Err("task is busy".to_string());
        }
        let Some(branch) = task.base_ref.clone() else {
            self.update_task(&p.id, |task| {
                task.record_decision(
                    "Land: refused, the task was not started from a branch; the work stays on its own branch"
                        .to_string(),
                );
            })?;
            return Err(
                "the task was not started from a branch, so there is nothing to land on"
                    .to_string(),
            );
        };
        let all = self.repo_tasks(&task.repo);
        let redirect = match crate::task::graph::landing::resolve_landing_branch(&branch, &all) {
            Ok(redirect) => redirect,
            Err(reason) => {
                self.update_task(&p.id, |task| {
                    task.record_decision(reason.clone());
                })?;
                return Err(reason);
            }
        };
        let branch = self.checked_landing_branch(&task, redirect).await?;
        if task.attempts.is_empty() && !is_parent(&task, &all) {
            return Err("the task has no finished attempt to land".to_string());
        }
        self.update_task(&p.id, |task| {
            let mut variant = task.variant();
            variant.land = true;
            task.apply_variant(variant);
            task.resume_phase(TaskStatus::Landing, crate::clock::now_ms());
            task.record_unlanded(None);
            // The owner chose landing: the automatic pull request is off. One
            // that is open stays open.
            task.finish_pr();

            if let Some(url) = &task.pr_url {
                task.record_decision(format!(
                    "Land: requested while pull request {url} is open; it stays open"
                ));
            }
            task.record_decision(format!("Land: requested, landing on {branch}"));
        })?;
        self.spawn_task_loop(p.id.clone(), true);
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }
}

impl App {
    /// Works in every state, including a task still queued behind the
    /// concurrency limit or parked waiting for an answer -- both select
    /// against the same `CancelToken`.
    pub(in crate::engine) async fn handle_task_stop(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            /// Why the task is stopped; optional.
            #[serde(default)]
            reason: Option<String>,
            #[serde(default)]
            source: AnswerSource,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if !matches!(task.status, TaskStatus::Done | TaskStatus::Failed)
            && (task.status != TaskStatus::Stopped || !task.queue.stopped_explicitly)
        {
            let waiting_to_land = task.status == TaskStatus::Landing;
            let line = p.source.control_line(
                "stop",
                p.reason
                    .as_deref()
                    .map(str::trim)
                    .map(|r| truncate_chars(r, 300))
                    .as_deref(),
            );
            self.close_stopped_task(&mut task);
            self.interrupt_task(&mut task, None, true, line)?;
            self.stop_children(&task.id, &task.repo);
            if waiting_to_land && !self.controls.lock().unwrap().contains_key(&p.id) {
                self.release_worktree(&mut task, "stopped while waiting to land")
                    .await;
                self.store.save_task(&task).map_err(|e| e.to_string())?;
            }
        } else if task.status == TaskStatus::Stopped && task.take_question().is_some() {
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
        }
        self.advance_queue(&task.repo);
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

impl App {
    /// `task.preset {id, preset}`: writes a preset into the task's variant
    /// before its first implement attempt.
    pub(in crate::engine) async fn handle_task_preset(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            preset: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let settings = self.settings.read().unwrap().clone();
        let preset = settings
            .preset(&p.preset)
            .ok_or_else(|| format!("unknown preset \"{}\"", p.preset))?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.archived {
            return Err("task is archived".to_string());
        }
        if implement_attempt_count(&task) > 0 {
            return Err(
                "the task has already run an implement attempt; a preset can only be set before it"
                    .to_string(),
            );
        }
        // A live loop keeps its own copy of the task and would overwrite
        // this change with its next save.
        if self
            .controls
            .lock()
            .unwrap()
            .get(&p.id)
            .is_some_and(|c| !c.signals.cancel.is_cancelled())
        {
            return Err("the task is running: stop it before changing its preset".to_string());
        }
        let mut variant = task.variant();
        preset.values.apply(&mut variant);
        variant.check()?;
        variant.check_routes(&settings.routes)?;
        let task = self.update_task(&p.id, |task| task.apply_variant(variant))?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

impl App {
    async fn checked_landing_branch(
        &self,
        task: &crate::task::Task,
        redirect: crate::task::graph::landing::LandingRedirect,
    ) -> Result<String, String> {
        let branch = redirect.branch.clone();
        let (land_on_default, repo) = (
            self.settings
                .read()
                .unwrap()
                .may_land_on_default(&task.repo),
            PathBuf::from(&task.repo),
        );
        if !land_on_default {
            let default =
                tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
                    .await
                    .map_err(|e| e.to_string())?;
            if default.as_deref() == Some(branch.as_str()) {
                let reason = crate::policy::delivery::land_on_default_refusal(&branch, &task.repo);
                self.update_task(&task.id, |task| {
                    task.record_decisions(redirect.decisions);
                    task.record_decision(format!(
                        "Land: refused, {reason} The work stays on {}",
                        task.branch
                    ));
                })?;
                return Err(reason);
            }
        }
        Ok(branch)
    }
}
