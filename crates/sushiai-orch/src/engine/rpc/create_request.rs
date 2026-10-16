use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::backlog::BacklogIn;
use crate::engine::routing::resolve_variant;
use crate::engine::secrets;
use crate::engine::state::App;
use crate::policy::delivery::BaseFrom;
use crate::store;
use crate::task::creation::NewTask;
use crate::task::valid_check;
use crate::task::valid_checks;
use crate::task::Check;
use crate::task::Task;
use crate::task::TASK_SOURCES;
use crate::verify::command_gate::{gate_checks, gate_held_out};
use serde::Deserialize;
use std::path::PathBuf;

use crate::policy::settings::Settings;
use crate::policy::variant::Variant;
#[derive(Deserialize)]
pub(super) struct CreateRequest {
    repo: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    criteria: Vec<String>,
    #[serde(default)]
    verify: Vec<String>,
    /// Slow checks run once, after review passes.
    #[serde(default, rename = "finalVerify")]
    final_verify: Vec<String>,
    /// Executable checks per criterion (`variant.groundedChecks`).
    #[serde(default)]
    checks: Vec<Check>,
    #[serde(default, rename = "heldOut")]
    held_out: Option<Check>,
    /// Repo-relative files or directories the task edits; it waits
    /// while another live task on the same base holds any of them.
    #[serde(default)]
    paths: Vec<String>,
    /// The repo command that captures screenshot evidence.
    #[serde(default)]
    screenshot: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    /// Commit-ish the task branches from; the repo's HEAD if unset.
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    mcp: Option<serde_json::Value>,
    #[serde(default, rename = "projectId")]
    project_id: Option<String>,
    #[serde(default)]
    start: Option<bool>,
    /// The `{repo, request}` form: a one-sentence ask instead of a
    /// filled-in title/goal/criteria/verify, drafted by the plan
    /// stage before anything is queued (spec "drafting stage").
    #[serde(default)]
    request: Option<String>,
    /// Experiment flags laid over `settings.experiments` for this
    /// task only (an A/B arm).
    #[serde(default)]
    variant: Option<serde_json::Value>,
    /// A preset id laid over `settings.experiments` before `variant`;
    /// the repo's `repoPresets` entry when unset.
    #[serde(default)]
    preset: Option<String>,
    /// Land the finished task on its base branch by itself; the same
    /// as `variant.land`.
    #[serde(default)]
    land: Option<bool>,
    /// Set by `orchd eval run`: the eval set and the task's name in it.
    #[serde(default, rename = "evalSet")]
    eval_set: Option<String>,
    #[serde(default, rename = "evalName")]
    eval_name: Option<String>,
    /// The set entry's `check`: grades the task's final commit.
    #[serde(default, rename = "evalCheck")]
    eval_check: Option<String>,
    /// Ids of tasks that must be done before this one implements.
    #[serde(default, rename = "dependsOn")]
    depends_on: Vec<String>,
    /// The task this one is a part of; it branches from and lands
    /// on that task's branch.
    #[serde(default)]
    parent: Option<String>,
    /// Where the work started (`TASK_SOURCES`); derived from the
    /// caller when unset.
    #[serde(default)]
    source: Option<String>,
    /// Puts the task in the planning backlog instead of starting it.
    #[serde(default)]
    backlog: Option<BacklogIn>,
}

impl CreateRequest {
    pub(super) async fn prepare(&self, app: &App) -> Result<NewTask, String> {
        if self.backlog.is_some() && self.parent.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return Err("a subtask cannot be in the backlog".to_string());
        }
        let request_text = self.request.clone().filter(|s| !s.trim().is_empty());
        let settings = app.settings.read().unwrap().clone();
        let variant = self.variant(&settings, request_text.is_some())?;
        let title = match (&request_text, &self.title) {
            (Some(r), _) => truncate_chars(r, 60),
            (None, Some(t)) => t.clone(),
            (None, None) => {
                return Err("task.create requires either request or title/goal".to_string())
            }
        };
        let goal = if request_text.is_some() {
            String::new()
        } else {
            self.goal
                .clone()
                .ok_or_else(|| "task.create requires goal".to_string())?
        };

        let repo_input = PathBuf::from(&self.repo);
        let repo_root =
            tokio::task::spawn_blocking(move || crate::git::history::repo_toplevel(&repo_input))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        let repo = repo_root.to_string_lossy().to_string();
        let project_id = secrets::project_for_repo(
            &app.secrets.read().unwrap(),
            &repo,
            self.project_id.as_deref(),
        )?;
        // Checked before anything is created: a rejected graph leaves no
        // worktree, branch or record behind.
        let (depends_on, parent) =
            app.check_graph_params(&repo, &self.depends_on, self.parent.as_deref(), None)?;
        let depends_on: Vec<String> = depends_on.into_iter().map(|t| t.id.clone()).collect();
        let (base, base_from) = self.base(app, &repo, parent.as_ref())?;
        let source = self.source()?;
        let backlog = match &self.backlog {
            Some(b) => Some(app.backlog_slot(&repo, None, b.bucket, b.order)?),
            None => None,
        };
        let mut refused = Vec::new();
        let checks = gate_checks(
            &repo_root,
            valid_checks(self.checks.clone(), self.criteria.len()),
            &mut refused,
        );
        let held_out = gate_held_out(
            &repo_root,
            valid_check(self.held_out.clone(), self.criteria.len()),
            &mut refused,
        );
        Ok(NewTask {
            id: uuid::Uuid::new_v4().to_string(),
            backlog,
            repo_root,
            project_id,
            title,
            decisions: refused,
            goal,
            checks,
            held_out,
            criteria: self.criteria.clone(),
            verify: self.verify.clone(),
            final_verify: self.final_verify.clone(),
            request: request_text.clone(),
            branch: self.branch.clone(),
            base,
            base_from,
            variant,
            depends_on,
            parent: parent.map(|t| t.id.clone()),
            paths: self
                .paths
                .iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            checklist: vec![],
            relay_of: None,
            eval_set: self.eval_set.clone().filter(|s| !s.trim().is_empty()),
            eval_name: self.eval_name.clone().filter(|s| !s.trim().is_empty()),
            eval_check_cmd: self.eval_check.clone().filter(|s| !s.trim().is_empty()),
            source: Some(source),
            created_at: now_ms(),
        })
    }
    fn variant(&self, settings: &Settings, drafting: bool) -> Result<Variant, String> {
        let preset = match self
            .preset
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => Some(
                settings
                    .preset(id)
                    .ok_or_else(|| format!("unknown preset \"{id}\""))?,
            ),
            None => settings.repo_preset(&self.repo),
        };
        let mut base_variant = settings.experiments.clone();
        if let Some(preset) = &preset {
            preset.values.apply(&mut base_variant);
        }
        let mut variant = resolve_variant(&base_variant, self.variant.as_ref())?;
        if let Some(land) = self.land {
            variant.land = land;
        }
        variant.check_routes(&settings.routes)?;

        if drafting {
            let planner = variant.plan_route_id(settings);
            if planner.is_empty() {
                return Err(
                    "planner is disabled; task.create needs title/goal instead of request"
                        .to_string(),
                );
            }
            // Same "off, not a silent fallback" rule `run_plan_stage` applies
            // at drafting time -- reject up front instead of creating a
            // worktree for a task that can only ever fail to plan.
            let known = settings.routes.iter().any(|r| r.id == planner);
            if !known {
                return Err(format!("planner route \"{planner}\" is not configured"));
            }
        }
        Ok(variant)
    }
    pub(super) fn activate(&self, app: &App, mut task: Task) -> Result<Task, String> {
        if let Some(cmd) = self.screenshot.as_ref().filter(|c| !c.trim().is_empty()) {
            task = app.update_task(&task.id, |task| {
                task.record_screenshot_command(Some(cmd.trim().to_string()));
            })?;
        }
        let id = task.id.clone();
        if let Some(mcp) = &self.mcp {
            let path = app.store.task_dir(&id).join("mcp.json");
            store::files::write_json_atomic(&path, mcp).map_err(|e| e.to_string())?;
        }
        app.broadcast_task(&task);
        if task.queue.backlog.is_some() {
            // Queued until `task.start` or the autopilot, with no loop and no
            // plan: it is planned on the base as it is then.
            app.advance_autopilot();
        } else if task.request.is_some() {
            // Planning always runs regardless of `start`; only whether the
            // loop falls through into implementing once it's done depends
            // on it (spec step 6).
            app.spawn_task_loop(id.clone(), self.start.unwrap_or(true));
        } else if self.start.unwrap_or(true) || task.parent.is_some() {
            // Same default as the request form: a `queued` task that never
            // starts until a daemon restart looks like work in progress. A
            // subtask starts on its own: its loop waits for what it depends
            // on. A top-level task with `start: false` gets no loop even when
            // it depends on others: the owner may still add subtasks to it,
            // and `advance_graph` starts it once its dependencies are done.
            app.start_task_loop(id.clone());
        }
        Ok(task)
    }
}

impl CreateRequest {
    fn base(
        &self,
        app: &App,
        repo: &str,
        parent: Option<&Task>,
    ) -> Result<(String, BaseFrom), String> {
        let base = self.base.clone().filter(|b| !b.trim().is_empty());
        Ok(match (parent, base) {
            (Some(parent), Some(base)) if base != parent.branch => {
                return Err(format!(
                    "a subtask starts from its parent's branch {}, not {base}",
                    parent.branch
                ))
            }
            (Some(parent), _) => (parent.branch.clone(), BaseFrom::Plain),
            (None, Some(base)) => (base, BaseFrom::Given),
            (None, None) => app.default_base(repo),
        })
    }
    fn source(&self) -> Result<String, String> {
        Ok(
            match self
                .source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(s) if TASK_SOURCES.contains(&s) => s.to_string(),
                Some(s) => {
                    return Err(format!(
                        "unknown source {s:?} ({})",
                        TASK_SOURCES.join(", ")
                    ))
                }
                None if self.eval_set.is_some() => "eval".to_string(),
                None => "cli".to_string(),
            },
        )
    }
}
