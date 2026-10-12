use super::*;

/// The most lines `task.log` returns: the last ones.
const TASK_LOG_MAX_LINES: usize = 10_000;

/// Everything `create_task_record` needs for a new task.
pub(super) struct NewTask {
    pub(super) id: String,
    pub(super) backlog: Option<Backlog>,
    pub(super) repo_root: PathBuf,
    pub(super) project_id: Option<String>,
    pub(super) title: String,
    pub(super) goal: String,
    pub(super) criteria: Vec<String>,
    pub(super) verify: Vec<String>,
    pub(super) final_verify: Vec<String>,
    pub(super) checks: Vec<Check>,
    pub(super) held_out: Option<Check>,
    pub(super) request: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) base: String,
    pub(super) variant: Variant,
    pub(super) depends_on: Vec<String>,
    pub(super) parent: Option<String>,
    pub(super) paths: Vec<String>,
    /// The sibling subtask whose branch this one continues.
    pub(super) relay_of: Option<String>,
    /// The ordered parts a merged task checks after every attempt.
    pub(super) checklist: Vec<ChecklistItem>,
    pub(super) eval_set: Option<String>,
    pub(super) eval_name: Option<String>,
    pub(super) eval_check_cmd: Option<String>,
    pub(super) source: Option<String>,
    pub(super) created_at: i64,
}

impl App {
    pub(super) async fn handle_task_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            repo: Option<String>,
            #[serde(default)]
            include_archived: bool,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut tasks = self.store.list_tasks().map_err(|e| e.to_string())?;
        if let Some(repo) = p.repo {
            tasks.retain(|t| t.repo == repo);
        }
        if !p.include_archived {
            tasks.retain(|t| !t.archived);
        }
        serde_json::to_value(&tasks).map_err(|e| e.to_string())
    }

    pub(super) async fn handle_task_get(
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
        let mut value = serde_json::to_value(&task).map_err(|e| e.to_string())?;
        // What each stage of this task cost, from the per-run cost records.
        let by_stage = crate::costs::summarize(
            &crate::costs::read_all(&self.data_dir),
            &crate::costs::Query {
                task_id: Some(task.id.clone()),
                group_by: vec!["stage".into()],
                ..Default::default()
            },
            now_ms(),
        );
        value["costByStage"] = by_stage["rows"].clone();
        Ok(value)
    }

    /// One saved evidence image of a task as a data URL, for a renderer that
    /// cannot read the data directory. Only paths an attempt lists.
    pub(super) async fn handle_task_evidence(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            path: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if !task.attempts.iter().any(|a| a.evidence.contains(&p.path)) {
            return Err("not an evidence image of this task".to_string());
        }
        let bytes = std::fs::read(&p.path).map_err(|e| e.to_string())?;
        let mime = crate::model::image_mime(Path::new(&p.path))
            .ok_or_else(|| "not an image".to_string())?;
        Ok(serde_json::json!({
            "dataUrl": format!("data:{mime};base64,{}", base64(&bytes)),
        }))
    }

    /// `task.log {id, attempt, stage}` -> `{lines, truncated}`: the stored
    /// run of a finished attempt's `plan`, `implement`, `review` or `advisor`
    /// stage, replayed into the same lines the live `log` event carried.
    pub(super) async fn handle_task_log(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            attempt: u32,
            stage: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let wanted = if p.stage == "plan" {
            Stage::Plan
        } else {
            Stage::Implement
        };
        if !matches!(
            p.stage.as_str(),
            "plan" | "implement" | "review" | "advisor"
        ) {
            return Err(format!(
                "unknown stage {}: plan, implement, review or advisor",
                p.stage
            ));
        }
        let attempt = match task
            .attempts
            .iter()
            .find(|a| a.n == p.attempt && a.stage == wanted)
        {
            Some(a) => a,
            None if task.attempts.iter().any(|a| a.n == p.attempt) => {
                return Err(format!("attempt {} has no {} stage", p.attempt, p.stage));
            }
            None => return Err(format!("attempt {} not found", p.attempt)),
        };
        if attempt.status == AttemptStatus::Running {
            return Err("the attempt is still running".to_string());
        }
        let run_dir = self.store.run_dir(&p.id, p.attempt);
        let fingerprint_harness =
            |f: &Option<Fingerprint>| f.as_ref().map_or(attempt.harness, |f| f.harness);
        let (paths, harness) = match p.stage.as_str() {
            "plan" => (
                vec![run_dir.join("plan").join("events.jsonl")],
                attempt.harness,
            ),
            "implement" => (vec![run_dir.join("events.jsonl")], attempt.harness),
            "review" => (
                self.store
                    .helper_run_dirs(&p.id, "review", Some(p.attempt))
                    .into_iter()
                    .map(|dir| dir.join("events.jsonl"))
                    .collect(),
                fingerprint_harness(&attempt.review_fingerprint),
            ),
            _ => (
                self.store
                    .helper_run_dirs(&p.id, "advisor", Some(p.attempt))
                    .into_iter()
                    .map(|dir| dir.join("events.jsonl"))
                    .collect(),
                fingerprint_harness(&attempt.advisor_fingerprint),
            ),
        };
        let (lines, truncated) = tokio::task::spawn_blocking(move || {
            let mut lines = Vec::new();
            let mut found = false;
            for path in paths {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                found = true;
                let text = String::from_utf8_lossy(&bytes);
                let mut outcome = harness::RunOutcome::default();
                lines.extend(
                    text.lines()
                        .filter_map(|l| harness::feed_stream_line(harness, l, &mut outcome)),
                );
            }
            if !found {
                return Err("no stored log for this stage".to_string());
            }
            let truncated = lines.len() > TASK_LOG_MAX_LINES;
            if truncated {
                lines.drain(..lines.len() - TASK_LOG_MAX_LINES);
            }
            Ok::<_, String>((lines, truncated))
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(json!({"lines": lines, "truncated": truncated}))
    }

    pub(super) async fn handle_task_create(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
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
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        if p.backlog.is_some() && p.parent.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return Err("a subtask cannot be in the backlog".to_string());
        }
        let settings = self.settings.read().unwrap().clone();
        let preset = match p.preset.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Some(
                settings
                    .preset(id)
                    .ok_or_else(|| format!("unknown preset \"{id}\""))?,
            ),
            None => settings.repo_preset(&p.repo),
        };
        let mut base_variant = settings.experiments.clone();
        if let Some(preset) = &preset {
            preset.values.apply(&mut base_variant);
        }
        let mut variant = resolve_variant(&base_variant, p.variant.as_ref())?;
        if let Some(land) = p.land {
            variant.land = land;
        }
        variant.check_routes(&settings.routes)?;
        let request_text = p.request.clone().filter(|s| !s.trim().is_empty());
        if request_text.is_some() {
            let planner = variant.plan_route_id(&settings);
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
        let title = match (&request_text, &p.title) {
            (Some(r), _) => truncate_chars(r, 60),
            (None, Some(t)) => t.clone(),
            (None, None) => {
                return Err("task.create requires either request or title/goal".to_string())
            }
        };
        let goal = if request_text.is_some() {
            String::new()
        } else {
            p.goal
                .clone()
                .ok_or_else(|| "task.create requires goal".to_string())?
        };

        let repo_input = PathBuf::from(&p.repo);
        let repo_root = tokio::task::spawn_blocking(move || git::repo_toplevel(&repo_input))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let repo = repo_root.to_string_lossy().to_string();
        // Checked before anything is created: a rejected graph leaves no
        // worktree, branch or record behind.
        let (depends_on, parent) =
            self.check_graph_params(&repo, &p.depends_on, p.parent.as_deref(), None)?;
        let depends_on: Vec<String> = depends_on.into_iter().map(|t| t.id).collect();
        let base = p.base.clone().filter(|b| !b.trim().is_empty());
        let base = match (&parent, base) {
            (Some(parent), Some(base)) if base != parent.branch => {
                return Err(format!(
                    "a subtask starts from its parent's branch {}, not {base}",
                    parent.branch
                ))
            }
            (Some(parent), _) => parent.branch.clone(),
            (None, base) => base.unwrap_or_else(|| "HEAD".to_string()),
        };
        let source = match p.source.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) if TASK_SOURCES.contains(&s) => s.to_string(),
            Some(s) => {
                return Err(format!(
                    "unknown source {s:?} ({})",
                    TASK_SOURCES.join(", ")
                ))
            }
            None if p.eval_set.is_some() => "eval".to_string(),
            None => "cli".to_string(),
        };
        let in_graph = parent.is_some() || !depends_on.is_empty();
        let backlog = match &p.backlog {
            Some(b) => Some(self.backlog_slot(&repo, None, b.bucket, b.order)?),
            None => None,
        };
        let mut task = self
            .create_task_record(NewTask {
                id: uuid::Uuid::new_v4().to_string(),
                backlog,
                repo_root,
                project_id: p.project_id,
                title,
                goal,
                checks: valid_checks(p.checks, p.criteria.len()),
                held_out: valid_check(p.held_out, p.criteria.len()),
                criteria: p.criteria,
                verify: p.verify,
                final_verify: p.final_verify,
                request: request_text.clone(),
                branch: p.branch.clone(),
                base,
                variant,
                depends_on,
                parent: parent.map(|t| t.id),
                paths: p
                    .paths
                    .iter()
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect(),
                checklist: vec![],
                relay_of: None,
                eval_set: p.eval_set.clone().filter(|s| !s.trim().is_empty()),
                eval_name: p.eval_name.clone().filter(|s| !s.trim().is_empty()),
                eval_check_cmd: p.eval_check.clone().filter(|s| !s.trim().is_empty()),
                source: Some(source),
                created_at: now_ms(),
            })
            .await?;
        if let Some(cmd) = p.screenshot.as_ref().filter(|c| !c.trim().is_empty()) {
            task = self.update_task(&task.id, |task| {
                task.brief_check.screenshot = Some(cmd.trim().to_string());
            })?;
        }
        let id = task.id.clone();
        if let Some(mcp) = &p.mcp {
            let path = self.store.task_dir(&id).join("mcp.json");
            store::write_json_atomic(&path, mcp).map_err(|e| e.to_string())?;
        }
        self.broadcast_task(&task);
        if backlog.is_some() {
            // Queued until `task.start` or the autopilot, with no loop and no
            // plan: it is planned on the base as it is then.
            self.advance_autopilot();
        } else if request_text.is_some() {
            // Planning always runs regardless of `start`; only whether the
            // loop falls through into implementing once it's done depends
            // on it (spec step 6).
            self.spawn_task_loop(id.clone(), p.start.unwrap_or(true));
        } else if p.start.unwrap_or(true) || in_graph {
            // Same default as the request form: a `queued` task that never
            // starts until a daemon restart looks like work in progress. A
            // task in a graph starts on its own: its loop waits for what it
            // depends on.
            self.start_task_loop(id.clone());
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// `task.create`/`task.amend`'s graph: every id a task of this repo,
    /// the parent a top-level task that has not implemented anything itself
    /// and is not done, and no dependency cycle once the new task is added
    /// (a parent waits for its children, so a child depending on its own
    /// parent is one). Returns the deduplicated dependency records and parent.
    fn check_graph_params(
        &self,
        repo: &str,
        depends_on: &[String],
        parent: Option<&str>,
        existing: Option<&Task>,
    ) -> Result<(Vec<Task>, Option<Task>), String> {
        let load = |id: &str, what: &str| -> Result<Task, String> {
            validate_task_id(&self.store, id).map_err(|_| format!("unknown {what} {id}"))?;
            let task = self
                .store
                .load_task(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("unknown {what} {id}"))?;
            if task.repo != repo {
                return Err(format!("{what} {id} belongs to another repository"));
            }
            Ok(task)
        };
        let mut deps: Vec<Task> = Vec::new();
        for id in depends_on {
            let dependency = load(id, "dependency")?;
            if existing.is_some_and(|t| dependency.parent.as_deref() == Some(t.id.as_str())) {
                return Err("a task cannot depend on its own child".to_string());
            }
            if !deps.iter().any(|t| t.id == *id) {
                deps.push(dependency);
            }
        }
        let parent = match parent.filter(|p| !p.trim().is_empty()) {
            None => None,
            Some(id) => {
                let parent = load(id, "parent")?;
                if parent.parent.is_some() {
                    return Err("a subtask cannot have subtasks of its own".to_string());
                }
                if parent.status == TaskStatus::Done {
                    return Err("the parent task is already done".to_string());
                }
                if implement_attempt_count(&parent) > 0 {
                    return Err(
                        "the parent task has already run an implement attempt of its own"
                            .to_string(),
                    );
                }
                // A live loop may already be queued for a slot to implement
                // the whole request; only a drafting one re-checks after
                // planning.
                if parent.status == TaskStatus::Landing {
                    return Err("the parent task is already landing".to_string());
                }
                // A parent with parts already runs no implement attempt of its
                // own, so a live loop may take one more part.
                if parent.status != TaskStatus::Drafting
                    && self.controls.lock().unwrap().contains_key(&parent.id)
                    && !is_parent(&parent, &self.repo_tasks(repo))
                {
                    return Err(
                        "the parent task is already running; create it with start: false, add its subtasks, then start it"
                            .to_string(),
                    );
                }
                Some(parent)
            }
        };
        let new_id = "(new task)".to_string();
        let mut all = self.repo_tasks(repo);
        {
            let controls = self.controls.lock().unwrap();
            for task in &mut all {
                if let Some(ctrl) = controls.get(&task.id).filter(|c| !c.cancel.is_cancelled()) {
                    if let Some(deps) = ctrl
                        .pending_amend
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|a| a.depends_on.as_ref())
                    {
                        task.depends_on = deps.clone();
                    }
                }
            }
        }
        let mut edges = wait_edges(&all);
        let mut waits: Vec<String> = deps.iter().map(|t| t.id.clone()).collect();
        if let Some(task) = existing {
            waits.extend(
                all.iter()
                    .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
                    .map(|t| t.id.clone()),
            );
            edges.insert(task.id.clone(), waits);
        } else {
            edges.insert(new_id.clone(), waits);
            if let Some(parent) = &parent {
                edges.entry(parent.id.clone()).or_default().push(new_id);
            }
        }
        if has_cycle(&edges) {
            return Err("dependsOn would make a dependency cycle".to_string());
        }
        Ok((deps, parent))
    }

    /// Creates the task's branch and worktree and saves its record, status
    /// `drafting` for a request, `queued` otherwise. Shared by `task.create`
    /// and the planner's split into subtasks.
    pub(super) async fn create_task_record(&self, new: NewTask) -> Result<Task, String> {
        let repo_root = new.repo_root.clone();
        let title_for_branch = new.title.clone();
        let branch_opt = new.branch.clone();
        let base = new.base.clone();
        let wt_root = self.settings.read().unwrap().worktree_root.clone();
        let home = self.home.clone();
        let created = tokio::task::spawn_blocking(move || {
            let branch = branch_opt
                .unwrap_or_else(|| git::unique_branch_name(&repo_root, &title_for_branch));
            let wt_path = git::worktree_path(&repo_root, &home, &wt_root, &branch);
            if let Some(dir) = wt_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| git::GitError(e.to_string()))?;
                git::exclude_worktree_root(&repo_root, dir)
                    .map_err(|e| git::GitError(e.to_string()))?;
            }
            let base_ref = git::branch_of(&repo_root, &base);
            let created = git::create_worktree(&repo_root, &branch, &wt_path, &base)?;
            if let Err(e) = git::bootstrap_worktree(&repo_root, &created.path) {
                git::discard_worktree(&repo_root, &created.path, &branch);
                return Err(git::GitError(e.to_string()));
            }
            Ok::<_, git::GitError>((branch, created, base_ref))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let (branch, created, base_ref) = created;

        let task = Task {
            id: new.id,
            title: new.title,
            goal: new.goal,
            criteria: new.criteria,
            verify: new.verify,
            final_verify: new.final_verify,
            checks: new.checks,
            held_out: new.held_out,
            planner_tests: Default::default(),
            // A backlog task with a request is planned when it starts, on
            // the base as it is then.
            status: if new.request.is_some() && new.backlog.is_none() {
                TaskStatus::Drafting
            } else {
                TaskStatus::Queued
            },
            request: new.request,
            repo: new.repo_root.to_string_lossy().to_string(),
            project_id: new.project_id,
            worktree: created.path.to_string_lossy().to_string(),
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            unlanded: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch,
            base_sha: created.base_sha,
            base_ref,
            depends_on: new.depends_on,
            paths: new.paths,
            parent: new.parent,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            findings: vec![],
            archived: false,
            pr_url: None,
            overseer_woken: false,
            planned_tier: None,
            planned_by_parent: false,
            tier_fallback: None,
            variant: Some(new.variant),
            eval_set: new.eval_set,
            eval_name: new.eval_name,
            eval_check_cmd: new.eval_check_cmd,
            source: new.source,
            eval_check: None,
            brief_check: Default::default(),
            plan_gate: Default::default(),
            metrics: None,
            checklist: new.checklist,
            queue: QueueState {
                relay_of: new.relay_of,
                backlog: new.backlog,
                ..Default::default()
            },
            created_at: new.created_at,
            updated_at: new.created_at,
        };
        if let Err(e) = self.store.save_task(&task) {
            let _ = std::fs::remove_dir_all(self.store.task_dir(&task.id));
            let (repo, wt, branch) = (
                task.repo.clone(),
                task.worktree.clone(),
                task.branch.clone(),
            );
            let _ = tokio::task::spawn_blocking(move || {
                git::discard_worktree(Path::new(&repo), Path::new(&wt), &branch)
            })
            .await;
            return Err(e.to_string());
        }
        Ok(task)
    }

    pub(super) async fn handle_task_start(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let already_running = self
            .controls
            .lock()
            .unwrap()
            .get(&p.id)
            .is_some_and(|c| !c.cancel.is_cancelled());
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

    /// `task.land {id}`: lands a done, unlanded top-level task now, through
    /// the same landing queue a `variant.land` task uses. The task goes back
    /// to `landing` and its loop does the rest; the default branch is still
    /// refused unless the repo is allowed in `settings.landOnDefaultRepos` (or `settings.landOnDefault`).
    pub(super) async fn handle_task_land(
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
                task.decisions.push(
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
        let redirect = match super::land::resolve_landing_branch(&branch, &all) {
            Ok(redirect) => redirect,
            Err(reason) => {
                self.update_task(&p.id, |task| {
                    task.decisions.push(reason.clone());
                })?;
                return Err(reason);
            }
        };
        let branch = redirect.branch.clone();
        let (land_on_default, repo) = (
            self.settings
                .read()
                .unwrap()
                .may_land_on_default(&task.repo),
            PathBuf::from(&task.repo),
        );
        if !land_on_default {
            let default = tokio::task::spawn_blocking(move || git::default_branch(&repo))
                .await
                .map_err(|e| e.to_string())?;
            if default.as_deref() == Some(branch.as_str()) {
                let reason = crate::model::land_on_default_refusal(&branch, &task.repo);
                self.update_task(&p.id, |task| {
                    task.decisions.extend(redirect.decisions);
                    task.decisions.push(format!(
                        "Land: refused, {reason} The work stays on {}",
                        task.branch
                    ));
                })?;
                return Err(reason);
            }
        }
        if task.attempts.is_empty() && !is_parent(&task, &all) {
            return Err("the task has no finished attempt to land".to_string());
        }
        self.update_task(&p.id, |task| {
            let mut variant = task.variant();
            variant.land = true;
            task.variant = Some(variant);
            task.status = TaskStatus::Landing;
            task.unlanded = None;
            task.decisions
                .push(format!("Land: requested, landing on {branch}"));
        })?;
        self.spawn_task_loop(p.id.clone(), true);
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// Works in every state, including a task still queued behind the
    /// concurrency limit or parked waiting for an answer -- both select
    /// against the same `CancelToken`.
    pub(super) async fn handle_task_stop(
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
        } else if task.status == TaskStatus::Stopped && task.question.take().is_some() {
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
        }
        self.advance_queue(&task.repo);
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// `task.overturn {id, index, answer}`: the owner disagrees with one of
    /// the planner's assumptions. It is marked overturned and, unless the
    /// task is done, the answer reaches the task's next attempt as a message.
    pub(super) async fn handle_task_overturn(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            index: usize,
            answer: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let answer = p.answer.trim();
        if answer.is_empty() {
            return Err("answer is required".to_string());
        }
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let assumption = task
            .assumptions
            .get(p.index)
            .ok_or_else(|| format!("task has no assumption {}", p.index))?;
        let text = format!(
            "The owner overturned an assumption made for them ({}). Question: {}\nThe assumption: {}\nThe owner's answer: {answer}\nUse the owner's answer.",
            assumption.by, assumption.question, assumption.answer
        );
        if task.status != TaskStatus::Done {
            messages::send(self, ORCHESTRATOR, Some(&task.id), &text, None)?;
        }
        let task = self.update_task(&p.id, |task| {
            if let Some(assumption) = task.assumptions.get_mut(p.index) {
                assumption.overturned = true;
                assumption.owner_answer = Some(answer.to_string());
            }
        })?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    pub(super) async fn handle_task_answer(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            answer: String,
            /// The bridge sets chat/mcp/overseer; raw RPC may set lead.
            /// Omitted means the owner in the app.
            #[serde(default)]
            source: Option<String>,
            /// With source `policy` (the waiting SLA): why this answer was given.
            #[serde(default)]
            evidence: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let caller = match p.source.as_deref() {
            None | Some("") => AnsweredBy::Owner,
            Some("lead" | "mcp") => AnsweredBy::Lead,
            Some("overseer") => AnsweredBy::Overseer,
            Some("chat") => AnsweredBy::Orchestrator,
            Some("policy") => AnsweredBy::Policy,
            Some(source) => return Err(format!("unknown answer source: {source}")),
        };
        let default_evidence = match (p.source.as_deref(), &p.evidence) {
            (Some("policy"), Some(e)) => Some(e.clone()),
            (Some("policy"), None) => return Err("a policy answer needs its evidence".to_string()),
            _ => None,
        };
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Waiting || task.question.is_none() {
            return Err("task is not waiting for an answer".to_string());
        }
        if caller != AnsweredBy::Owner {
            if let Some(q) = task.question.as_ref().filter(|q| owner_only(q.kind)) {
                if !is_refusal(q, &p.answer) {
                    return Err(format!(
                        "this {:?} question is the owner's to allow: an agent can only stop or deny it",
                        q.kind
                    ));
                }
            }
        }

        let by_agent = caller != AnsweredBy::Owner;
        if by_agent {
            if let Some(q) = task.question.as_ref() {
                let settings = self.settings.read().unwrap();
                if let Some(why) = shared_check_drop_refusal(&settings, &task, q, &p.answer) {
                    return Err(why);
                }
            }
            if is_accept_option(&p.answer) {
                return Err(
                    "accepting an attempt as done is the owner's to pick: an agent cannot"
                        .to_string(),
                );
            }
            let exhausted = task
                .question
                .as_ref()
                .is_some_and(|q| q.kind == QuestionKind::AttemptsFailing);
            let continues = non_owner_continues(&task);
            if exhausted
                && continues >= MAX_ORCHESTRATOR_CONTINUES
                && !p.answer.trim().eq_ignore_ascii_case("stop")
            {
                return Err(
                    "this task already got the orchestrator's continues: the owner decides now"
                        .to_string(),
                );
            }
            if let Some(q) = task.question.as_ref().filter(|q| {
                q.kind == QuestionKind::PreexistingFailure
                    && !q.commands.is_empty()
                    && is_option(&p.answer, PRE_EXISTING_RETRY)
            }) {
                if q.commands
                    .iter()
                    .all(|command| non_owner_base_retry(&task, command, Some(&task.base_sha)))
                {
                    return Err("these checks already got a non-owner retry on this base: the owner decides now".to_string());
                }
            }
            self.agent_answers
                .lock()
                .unwrap()
                .insert(p.id.clone(), caller);
            // Only a turn orchd woke is the overseer; the owner's chat is not.
            if p.source.as_deref() == Some("overseer") {
                self.record_overseer(overseer::Entry::answered(
                    &task.repo,
                    &task.id,
                    task.question.as_ref(),
                    &p.answer,
                ));
            }
        } else {
            self.agent_answers.lock().unwrap().remove(&p.id);
        }

        if p.answer.trim().eq_ignore_ascii_case("stop") {
            self.agent_answers.lock().unwrap().remove(&p.id);
            let question = task.question.take();
            record_answer_for(
                &mut task,
                question.as_ref(),
                "stop",
                caller,
                default_evidence.as_deref(),
            );
            let line = match caller {
                AnsweredBy::Owner => "Owner: stop (UI)".to_string(),
                AnsweredBy::Lead => "Lead: stop (RPC)".to_string(),
                _ => task.decisions.last().cloned().unwrap_or_default(),
            };
            if matches!(caller, AnsweredBy::Owner | AnsweredBy::Lead) {
                if let Some(last) = task.decisions.last_mut() {
                    *last = line.clone();
                }
            }
            self.close_stopped_task(&mut task);
            self.interrupt_task(&mut task, None, true, line)?;
            self.stop_children(&task.id, &task.repo);
            self.advance_graph(&task.repo);
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }

        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
        {
            self.agent_answers.lock().unwrap().remove(&p.id);
            return self.answer_dependency_question(
                task,
                &p.answer,
                Some(caller),
                default_evidence.as_deref(),
            );
        }

        // A tool call held for this question: the run goes on with the answer.
        if let Some(tx) = self.permission_waits.lock().unwrap().remove(&p.id) {
            if tx
                .send(Reply {
                    text: p.answer.clone(),
                    default_evidence: default_evidence.clone(),
                })
                .is_ok()
            {
                return serde_json::to_value(&task).map_err(|e| e.to_string());
            }
        }

        let (delivered, has_control) = {
            let controls = self.controls.lock().unwrap();
            let control = controls.get(&p.id);
            let delivered = control
                .and_then(|c| c.pending_answer.lock().unwrap().take())
                .map(|tx| {
                    tx.send(Reply {
                        text: p.answer.clone(),
                        default_evidence: default_evidence.clone(),
                    })
                    .is_ok()
                })
                .unwrap_or(false);
            (delivered, control.is_some())
        };
        // A default must not relaunch a loop whose question is being answered.
        if !delivered && default_evidence.is_some() && has_control {
            self.agent_answers.lock().unwrap().remove(&p.id);
            return Err("the question was answered meanwhile".to_string());
        }
        if !delivered {
            // No live parked loop (e.g. after a daemon restart): apply the
            // decision synchronously and relaunch (spec step 9: "status
            // queued, loop continues").
            self.update_task(&p.id, |task| {
                let question = task.question.clone();
                record_answer_for(
                    task,
                    question.as_ref(),
                    &p.answer,
                    caller,
                    default_evidence.as_deref(),
                );
                self.agent_answers.lock().unwrap().remove(&p.id);
                if let Some(n) = task
                    .question
                    .as_ref()
                    .and_then(|q| impossible_drop_target(q, &p.answer))
                {
                    let settings = self.settings.read().unwrap().clone();
                    let origin = origin_of_last_answer(task);
                    let _ = drop_criterion(&settings, task, n, origin);
                }
                if let Some(q) = task.question.clone() {
                    // A base-check question: the task starts again only with the
                    // answer applied, and the owner's answer is remembered.
                    if q.kind == QuestionKind::PreexistingFailure && !q.commands.is_empty() {
                        let base_sha = task.base_sha.clone();
                        record_owner_base_check(
                            task,
                            &q.commands,
                            &base_sha,
                            &p.answer,
                            default_evidence.is_some(),
                        );
                        let settings = self.settings.read().unwrap().clone();
                        let origin = origin_of_last_answer(task);
                        apply_base_check_answer(&settings, task, &q, &p.answer, origin);
                        if is_option(&p.answer, PRE_EXISTING_DROP) {
                            self.verify_cache.lock().unwrap().remove(&task.id);
                        }
                    }
                }
                // The owner accepted the last attempt: the relaunched loop
                // commits it instead of starting another (`pending_acceptance`).
                if let Some(q) = task.question.clone().filter(|q| {
                    q.kind == QuestionKind::AttemptsFailing
                        && is_accept_option(&p.answer)
                        && q.options.iter().any(|o| is_accept_option(o))
                }) {
                    if let Some(a) = task
                        .attempts
                        .iter()
                        .rfind(|a| a.stage == Stage::Implement)
                        .filter(|a| {
                            matches!(
                                a.failure.as_ref().map(|f| f.kind),
                                Some(FailureKind::Review | FailureKind::Evidence)
                            )
                        })
                    {
                        let n = a.n;
                        record_owner_accept(task, &q.text, &p.answer, "owner picked the option", n);
                    }
                }
                if task
                    .question
                    .as_ref()
                    .is_some_and(|q| is_budget_raise(q, &p.answer))
                {
                    task.budget_raises += 1;
                }
                if task
                    .question
                    .as_ref()
                    .is_some_and(|q| is_daily_budget_run_anyway(q, &p.answer))
                {
                    task.daily_budget_ok_day = Some(crate::costs::today(now_ms()));
                }
                task.question = None;
                task.status = TaskStatus::Queued;
            })?;
            self.start_task_loop(p.id.clone());
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// Owner amendment of a task's brief, checks or dependencies. A task
    /// with a live loop (running, or parked on a question) gets it through
    /// `pending_amend`, because the loop saves its own copy of the task and
    /// would overwrite a write made here; any other task is saved directly.
    pub(super) async fn handle_task_amend(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        fn field<T: serde::de::DeserializeOwned>(
            params: &serde_json::Value,
            name: &str,
            what: &str,
        ) -> Result<Option<T>, String> {
            match params.get(name) {
                None => Ok(None),
                Some(v) => serde_json::from_value(v.clone())
                    .map(Some)
                    .map_err(|_| format!("{name} must be {what}")),
            }
        }
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "id must be a string".to_string())?
            .to_string();
        let held_out = match params.get("heldOut") {
            None => None,
            Some(serde_json::Value::Null) => Some(None),
            Some(v) => Some(Some(
                serde_json::from_value::<Check>(v.clone())
                    .map_err(|_| "heldOut must be an object {criterion, run} or null")?,
            )),
        };
        let (criteria, visual_criteria) = match params.get("criteria") {
            None => (None, Vec::new()),
            Some(v) => {
                let bad = || "criteria must be an array of strings or {text, visual} objects";
                let items = v.as_array().ok_or_else(bad)?;
                let mut texts = Vec::new();
                let mut visual = Vec::new();
                for item in items {
                    match item {
                        serde_json::Value::String(s) => texts.push(s.clone()),
                        serde_json::Value::Object(o) => {
                            let text = o.get("text").and_then(|t| t.as_str()).ok_or_else(bad)?;
                            if o.get("visual").and_then(|f| f.as_bool()) == Some(true) {
                                visual.push(text.to_string());
                            }
                            texts.push(text.to_string());
                        }
                        _ => return Err(bad().to_string()),
                    }
                }
                (Some(texts), visual)
            }
        };
        let mut amendment = Amendment {
            criteria,
            visual_criteria,
            verify: field(&params, "verify", "an array of strings")?,
            final_verify: field(&params, "finalVerify", "an array of strings")?,
            screenshot: field(&params, "screenshot", "a string")?,
            checks: field(&params, "checks", "an array of {criterion, run} objects")?,
            held_out,
            depends_on: field(&params, "dependsOn", "an array of strings")?,
            dependency_titles: Vec::new(),
        };
        if amendment.is_empty() {
            return Err(
                "task.amend needs at least one of criteria, verify, finalVerify, screenshot, checks, heldOut, dependsOn"
                    .to_string(),
            );
        }
        validate_task_id(&self.store, &id)?;
        let mut task = self
            .store
            .load_task(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if params
            .get("source")
            .and_then(|s| s.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            // Checked against what the task will be once an amendment still
            // pending is applied, so it cannot overwrite the owner's.
            let mut effective = task.clone();
            if let Some(ctrl) = self.controls.lock().unwrap().get(&id) {
                if let Some(pending) = ctrl.pending_amend.lock().unwrap().clone() {
                    pending.apply(&mut effective);
                }
            }
            let settings = self.settings.read().unwrap();
            if let Some(why) = shared_check_amend_refusal(&settings, &effective, &amendment) {
                return Err(why);
            }
        }
        if task.archived {
            return Err("task is archived".to_string());
        }
        if !matches!(
            task.status,
            TaskStatus::Drafting
                | TaskStatus::Running
                | TaskStatus::Waiting
                | TaskStatus::Queued
                | TaskStatus::Stopped
        ) {
            return Err(format!(
                "task is {:?}: only drafting, running, waiting, queued or stopped tasks can be amended",
                task.status
            )
            .to_lowercase());
        }
        if let Some(deps) = &amendment.depends_on {
            if implement_attempt_count(&task) > 0 {
                return Err("task has already run an implement attempt".to_string());
            }
            if self.repo_tasks(&task.repo).iter().any(|child| {
                child.parent.as_deref() == Some(task.id.as_str())
                    && !child.archived
                    && implement_attempt_count(child) > 0
            }) {
                return Err("a child has already run an implement attempt".to_string());
            }
            let (deps, _) = self.check_graph_params(&task.repo, deps, None, Some(&task))?;
            amendment.dependency_titles = deps.iter().map(|t| t.title.clone()).collect();
            amendment.depends_on = Some(deps.into_iter().map(|t| t.id).collect());
        }
        let criteria_len = amendment
            .criteria
            .as_ref()
            .map_or(task.criteria.len(), Vec::len);
        let stale = match &amendment.checks {
            Some(_) => None,
            None if amendment.criteria.is_some() => {
                task.checks.iter().find(|c| c.criterion >= criteria_len)
            }
            None => None,
        };
        let held = match &amendment.held_out {
            Some(h) => h.as_ref(),
            None if amendment.criteria.is_some() => task.held_out.as_ref(),
            None => None,
        };
        if let Some(c) = amendment
            .checks
            .iter()
            .flatten()
            .chain(stale)
            .chain(held)
            .find(|c| c.criterion >= criteria_len)
        {
            return Err(format!(
                "check criterion {} is out of range: there are {criteria_len} criteria",
                c.criterion
            ));
        }
        let fields = amendment.fields();
        if amendment.verify.is_some() {
            self.verify_cache.lock().unwrap().remove(&id);
        }

        // Under the controls lock a loop cannot finish (and stop reading its
        // slot) between the check and the hand-over.
        {
            let controls = self.controls.lock().unwrap();
            if let Some(ctrl) = controls.get(&id).filter(|c| !c.cancel.is_cancelled()) {
                let mut slot = ctrl.pending_amend.lock().unwrap();
                match slot.as_mut() {
                    Some(pending) => pending.merge(amendment),
                    None => *slot = Some(amendment),
                }
                return Ok(json!({"id": id, "amended": fields, "pending": true}));
            }
        }
        let dependencies_amended = amendment.depends_on.is_some();
        amendment.clone().apply(&mut task);
        baseline_on_base(&self.arc(), &mut task, &CancelToken::new()).await;
        let updated = self.update_task(&id, |fresh| {
            amendment.apply(fresh);
            if dependencies_amended
                && fresh.status == TaskStatus::Waiting
                && fresh
                    .question
                    .as_ref()
                    .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
            {
                fresh.question = None;
                fresh.status = TaskStatus::Queued;
            }
            for check in fresh.checks.iter_mut().chain(fresh.held_out.iter_mut()) {
                if let Some(baselined) = task.checks.iter().chain(task.held_out.iter()).find(|c| {
                    c.criterion == check.criterion && c.run == check.run && c.baseline.is_some()
                }) {
                    check.baseline = baselined.baseline;
                }
            }
            for decision in &task.decisions {
                if !fresh.decisions.contains(decision) {
                    fresh.decisions.push(decision.clone());
                }
            }
        })?;
        if dependencies_amended {
            if updated.status == TaskStatus::Queued && updated.queue.backlog.is_none() {
                self.start_task_loop(id.clone());
            }
            self.advance_graph(&updated.repo);
        }
        Ok(json!({"id": id, "amended": fields, "pending": false}))
    }

    /// `task.preset {id, preset}`: writes a preset into the task's variant
    /// before its first implement attempt.
    pub(super) async fn handle_task_preset(
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
            .is_some_and(|c| !c.cancel.is_cancelled())
        {
            return Err("the task is running: stop it before changing its preset".to_string());
        }
        let mut variant = task.variant();
        preset.values.apply(&mut variant);
        variant.check()?;
        variant.check_routes(&settings.routes)?;
        let task = self.update_task(&p.id, |task| task.variant = Some(variant))?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// Cancels the loop and waits for it to actually exit before touching
    /// the filesystem, so a still-running attempt can never write into a
    /// directory that's mid-deletion.
    pub(super) async fn handle_task_delete(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let removed = self.controls.lock().unwrap().remove(&p.id);
        if let Some(ctrl) = removed {
            ctrl.cancel.cancel();
            let _ = ctrl.handle.await;
        }
        let doomed = self.store.load_task(&p.id).ok().flatten();
        let repo = doomed.as_ref().map(|t| t.repo.clone());
        if let Some(t) = doomed {
            let _ = tokio::task::spawn_blocking(move || {
                git::remove_task_worktree(Path::new(&t.repo), Path::new(&t.worktree));
                git::delete_branch(Path::new(&t.repo), &t.branch, &t.id);
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

    /// Hides a task from the default `task.list` without touching its
    /// worktree, branch, or record -- refused while a loop could still be
    /// mutating it (running, drafting a plan, or parked waiting for an
    /// answer), same as `task.delete` would need to stop it first, except
    /// this is never destructive so there's nothing to reconcile after. Also
    /// refused while a loop is merely queued behind the concurrency limit
    /// (present in `controls` even though `status` still reads `Queued`) --
    /// that loop is still going to run and mutate the task the moment a
    /// slot frees up, same hazard as an already-running one.
    pub(super) async fn handle_task_archive(
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
            return Err("cannot archive a running, drafting, waiting or landing task".to_string());
        }
        if self.controls.lock().unwrap().contains_key(&p.id) {
            return Err("cannot archive a task with a live loop".to_string());
        }
        let decisions_before = task.decisions.len();
        self.release_worktree(&mut task, "archived").await;
        let task = self.update_task(&p.id, |fresh| {
            fresh.archived = true;
            fresh.worktree_removed = task.worktree_removed;
            fresh.deliverables = task.deliverables.clone();
            fresh
                .decisions
                .extend_from_slice(&task.decisions[decisions_before..]);
        })?;
        // An archived child no longer holds its parent back.
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    pub(super) async fn handle_task_unarchive(
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
                task.decisions.push(format!("Worktree: {e}"));
            }
        }
        let task = self.update_task(&p.id, |fresh| {
            fresh.archived = false;
            fresh.worktree_removed = task.worktree_removed;
            fresh
                .decisions
                .extend_from_slice(&task.decisions[decisions_before..]);
        })?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    fn waiting(app: &App, kind: QuestionKind, options: &[&str]) -> String {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.question = Some(Question::new(
            "Allow it?",
            options.iter().map(|o| o.to_string()).collect(),
            kind,
            AskedBy::Implement,
        ));
        app.store.save_task(&task).unwrap();
        task.id
    }

    fn park_permission(app: &App, id: &str) -> oneshot::Receiver<Reply> {
        let (tx, rx) = oneshot::channel();
        app.permission_waits
            .lock()
            .unwrap()
            .insert(id.to_string(), tx);
        rx
    }

    async fn answer(app: &App, id: &str, text: &str, source: Option<&str>) -> Result<(), String> {
        let mut params = json!({"id": id, "answer": text});
        if let Some(s) = source {
            params["source"] = json!(s);
            if s == "policy" {
                params["evidence"] = json!("waiting SLA elapsed");
            }
        }
        app.handle_task_answer(params).await.map(|_| ())
    }

    #[tokio::test]
    async fn an_mcp_allow_on_an_owner_only_question_is_refused_and_it_stays_pending() {
        let (app, _dir) = test_app();
        for (kind, options, allow) in [
            (
                QuestionKind::Permission,
                &["Allow once", "Always for this repo", "Deny"][..],
                "Always for this repo",
            ),
            (
                QuestionKind::ProtectedPath,
                &["approve", "reject"][..],
                "approve",
            ),
            (QuestionKind::Budget, &["raise", "stop"][..], "raise"),
            (
                QuestionKind::DailyBudget,
                &["run anyway", "stop"][..],
                "run anyway",
            ),
            (QuestionKind::HarnessMissing, &["retry"][..], "retry"),
        ] {
            let id = waiting(&app, kind, options);
            let mut rx = park_permission(&app, &id);
            for source in ["lead", "overseer", "mcp", "chat"] {
                let err = answer(&app, &id, allow, Some(source)).await.unwrap_err();
                assert!(err.contains("owner's to allow"), "{kind:?}: {err}");
            }
            let task = app.store.load_task(&id).unwrap().unwrap();
            assert_eq!(task.status, TaskStatus::Waiting, "{kind:?}");
            assert!(task.question.is_some(), "{kind:?} is still pending");
            assert!(rx.try_recv().is_err(), "{kind:?}: nothing was delivered");
        }
    }

    /// A waiting base-check question about `command`, the task's request being `request`.
    fn waiting_base_check(app: &App, command: &str, request: Option<&str>) -> String {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.request = request.map(str::to_string);
        task.question = Some(base_check::pre_existing_question(
            &[VerifyOutcome {
                command: command.to_string(),
                code: Some(1),
                tail: "boom".to_string(),
                ms: 1,
                ..Default::default()
            }],
            "0123456789",
        ));
        app.store.save_task(&task).unwrap();
        task.id
    }

    #[tokio::test]
    async fn a_non_owner_cannot_drop_a_shared_check_but_may_retry_or_stop() {
        let (app, _dir) = test_app();
        {
            let mut settings = app.settings.write().unwrap();
            settings.scoped_checks.push(ScopedCheck {
                repo: None,
                command: "make lint*".to_string(),
                paths: vec!["src/**".to_string()],
            });
        }
        let drop = base_check::PRE_EXISTING_DROP;
        for (command, request) in [
            ("npm run ci", None),
            ("  cargo   test ", None),
            ("make lint-all", None),
            (
                "./scripts/gate.sh --strict",
                Some("Fix the bug; run ./scripts/gate.sh --strict at the end"),
            ),
        ] {
            for source in ["lead", "overseer", "mcp", "chat"] {
                let id = waiting_base_check(&app, command, request);
                let err = answer(&app, &id, &format!("{drop}."), Some(source))
                    .await
                    .unwrap_err();
                assert!(err.contains("only the owner may drop"), "{command}: {err}");
                let task = app.store.load_task(&id).unwrap().unwrap();
                assert_eq!(task.status, TaskStatus::Waiting, "{command} still waits");
            }
        }
        // A waiting-SLA default (source policy) is refused the same way.
        let id = waiting_base_check(&app, "npm run ci", None);
        let err = app
            .handle_task_answer(
                json!({"id": id, "answer": drop, "source": "policy", "evidence": "waited"}),
            )
            .await
            .unwrap_err();
        assert!(err.contains("only the owner may drop"), "{err}");
        let id = waiting_base_check(&app, "npm run ci", None);
        answer(&app, &id, "stop", Some("overseer")).await.unwrap();
        assert_eq!(
            app.store.load_task(&id).unwrap().unwrap().status,
            TaskStatus::Stopped
        );
        // The owner (no source) may drop it, and an agent may drop a task-specific check.
        let id = waiting_base_check(&app, "npm run ci", None);
        answer(&app, &id, drop, None).await.unwrap();
        let id = waiting_base_check(&app, "node --test tests/planner-made.test.cjs", None);
        answer(&app, &id, drop, Some("overseer")).await.unwrap();
    }

    #[test]
    fn a_ci_entry_is_found_through_flags_cd_env_pipes_and_chains() {
        let (settings, task) = (Settings::default(), task_with_status(TaskStatus::Waiting));
        let shared = |c: &str| answer_policy::is_shared_check(&settings, &task, c);
        for command in [
            "npm run ci --silent",
            "cd web && cargo test",
            "CI=1 npm test",
            "npm run ci 2>&1 | tail -20",
            "echo start; yarn test",
            "cargo test -p sushiai-orch",
            "(cd web && npm test)",
            "env CI=1 npm test",
            "timeout 900 cargo test",
        ] {
            assert!(shared(command), "{command}");
        }
        for command in [
            "node --test tests/one.test.cjs",
            "npm run test:desktop",
            "echo npm run ci",
            "grep -r 'cargo test' docs",
        ] {
            assert!(!shared(command), "{command}");
        }
    }

    #[tokio::test]
    async fn an_agent_amendment_cannot_remove_a_shared_check_but_the_owner_can() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.verify = vec![
            "npm run ci".into(),
            "node --test tests/mine.test.cjs".into(),
        ];
        task.final_verify = vec!["cargo test".into()];
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        for (field, value) in [
            ("verify", json!(["node --test tests/mine.test.cjs"])),
            ("finalVerify", json!([])),
        ] {
            let params = json!({"id": id, field: value, "source": "overseer"});
            let err = app.handle_task_amend(params).await.unwrap_err();
            assert!(err.contains("only the owner may drop"), "{field}: {err}");
        }
        // Keeping the shared check while dropping a task-specific one is fine.
        let keep = json!({"id": id, "verify": ["npm run ci"], "source": "mcp"});
        app.handle_task_amend(keep).await.unwrap();
        // The owner (no source) may remove it.
        let owner = json!({"id": id, "finalVerify": []});
        app.handle_task_amend(owner).await.unwrap();
    }

    /// A waiting `Impossible` question about criterion 0, which carries `npm test`.
    fn waiting_impossible(app: &App) -> String {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.criteria = vec!["Tests pass -- check: npm test".into(), "Docs exist".into()];
        task.checks = vec![Check {
            criterion: 0,
            run: "npm test".into(),
            baseline: None,
        }];
        task.question = Some(Question::new(
            "Criterion 0 cannot be met as written: x. Evidence: y",
            vec!["drop criterion".into(), "retry".into(), "stop".into()],
            QuestionKind::Impossible,
            AskedBy::Implement,
        ));
        app.store.save_task(&task).unwrap();
        task.id
    }

    #[tokio::test]
    async fn dropping_a_criterion_that_carries_a_shared_check_is_the_owners() {
        let (app, _dir) = test_app();
        for source in ["lead", "overseer", "mcp", "chat"] {
            let id = waiting_impossible(&app);
            let err = answer(&app, &id, "drop criterion", Some(source))
                .await
                .unwrap_err();
            assert!(err.contains("only the owner may drop"), "{source}: {err}");
            let task = app.store.load_task(&id).unwrap().unwrap();
            assert_eq!((task.criteria.len(), task.checks.len()), (2, 1));
        }
        // Retrying is open to an agent.
        let id = waiting_impossible(&app);
        answer(&app, &id, "retry", Some("overseer")).await.unwrap();
        // The owner drops it: the criterion and its check go.
        let id = waiting_impossible(&app);
        answer(&app, &id, "drop criterion", None).await.unwrap();
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!((task.criteria.len(), task.checks.len()), (1, 0));
    }

    #[test]
    fn the_removal_choke_point_refuses_a_non_owner_for_every_kind_of_removal() {
        let settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.criteria = vec!["A -- check: cargo test".into()];
        task.verify = vec![
            "npm run ci".into(),
            "node --test tests/mine.test.cjs".into(),
        ];
        use answer_policy::Origin::{Other, Owner};
        assert!(decision::drop_criterion(&settings, &mut task, 0, Other).is_err());
        assert_eq!(task.criteria.len(), 1, "still there");
        assert!(
            answer_policy::remove_check_command(&settings, &mut task, "npm run ci", Other).is_err()
        );
        assert!(answer_policy::remove_check_command(
            &settings,
            &mut task,
            "node --test tests/mine.test.cjs",
            Other
        )
        .is_ok());
        assert_eq!(task.verify, vec!["npm run ci".to_string()]);
        let ruled = healing::apply_feasibility(
            &settings,
            &mut task,
            "npm run ci",
            healing::Feasibility::Unrelated { reason: "x".into() },
        );
        assert!(ruled && task.verify == vec!["npm run ci".to_string()]);
        assert!(task.brief_check.non_gating.is_empty());
        assert!(decision::drop_criterion(&settings, &mut task, 0, Owner).is_ok());
        assert!(task.criteria.is_empty());
    }

    #[tokio::test]
    async fn an_amendment_cannot_drop_a_shared_held_out_or_criterion_check_or_overwrite_a_pending_one(
    ) {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.criteria = vec!["A -- check: npm test".into(), "B".into()];
        task.held_out = Some(Check {
            criterion: 1,
            run: "cargo test".into(),
            baseline: None,
        });
        task.verify = vec!["node --test tests/mine.test.cjs".into()];
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        for params in [
            json!({"id": id, "heldOut": null, "source": "overseer"}),
            json!({"id": id, "heldOut": {"criterion": 1, "run": "true"}, "source": "overseer"}),
            json!({"id": id, "criteria": ["A", "B"], "source": "overseer"}),
            json!({"id": id, "criteria": ["A -- check: npm test"], "source": "mcp"}),
        ] {
            let err = app.handle_task_amend(params.clone()).await.unwrap_err();
            assert!(err.contains("only the owner may drop"), "{params}: {err}");
        }
        // Keeping them is fine.
        let keep =
            json!({"id": id, "criteria": ["A -- check: npm test", "B", "C"], "source": "overseer"});
        app.handle_task_amend(keep).await.unwrap();

        // The owner's pending amendment adds `npm run ci`; an agent's later
        // amendment of verify without it cannot overwrite it.
        let pending_slot = Arc::new(StdMutex::new(None));
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: pending_slot.clone(),
                handle: tokio::spawn(async {}),
            },
        );
        app.handle_task_amend(
            json!({"id": id, "verify": ["node --test tests/mine.test.cjs", "npm run ci"]}),
        )
        .await
        .unwrap();
        assert!(pending_slot.lock().unwrap().is_some());
        let err = app
            .handle_task_amend(json!({"id": id, "verify": ["node --test tests/mine.test.cjs"], "source": "overseer"}))
            .await
            .unwrap_err();
        assert!(err.contains("only the owner may drop"), "{err}");
    }

    #[test]
    fn a_judge_cannot_rewrite_a_criterion_to_lose_its_shared_check() {
        let settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.criteria = vec!["Works -- check: cargo test".into(), "Docs".into()];
        let rewrite = healing::Feasibility::Rewrite {
            command: "true".into(),
            criterion: Some((0, "Works -- check: grep -q ok f".into())),
            reason: "r".into(),
        };
        assert!(healing::apply_feasibility(
            &settings, &mut task, "x", rewrite
        ));
        assert_eq!(task.criteria[0], "Works -- check: cargo test");
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("only the owner may drop")));
        // A task-specific criterion can be reworded.
        task.criteria[1] = "Docs -- check: grep -q docs d.md".into();
        assert!(healing::amend_criterion_for_test(
            &settings,
            &mut task,
            1,
            "Docs -- check: test -f d.md"
        )
        .is_ok());
        // Keeping the shared command is fine.
        assert!(healing::amend_criterion_for_test(
            &settings,
            &mut task,
            0,
            "Works well -- check: cargo test"
        )
        .is_ok());
    }

    #[tokio::test]
    async fn an_after_land_command_is_a_shared_check_only_for_its_repo() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = "/repo/a".to_string();
        let settings = {
            let mut s = app.settings.write().unwrap();
            s.after_land.push(AfterLand {
                repo: "/repo/a".to_string(),
                run: "just rebuild".to_string(),
            });
            s.clone()
        };
        assert!(answer_policy::is_shared_check(
            &settings,
            &task,
            "just rebuild"
        ));
        task.repo = "/repo/b".to_string();
        assert!(!answer_policy::is_shared_check(
            &settings,
            &task,
            "just rebuild"
        ));
    }

    #[tokio::test]
    async fn an_mcp_deny_or_stop_is_accepted() {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::Permission, &["Allow once", "Deny"]);
        let rx = park_permission(&app, &id);
        answer(&app, &id, "Deny", Some("mcp")).await.unwrap();
        assert_eq!(rx.await.unwrap().text, "Deny");

        let id = waiting(&app, QuestionKind::ProtectedPath, &["approve", "reject"]);
        let rx = park_permission(&app, &id);
        answer(&app, &id, "reject", Some("chat")).await.unwrap();
        assert_eq!(rx.await.unwrap().text, "reject");

        let id = waiting(&app, QuestionKind::Budget, &["raise", "stop"]);
        answer(&app, &id, "stop", Some("mcp")).await.unwrap();
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn only_an_overseer_answer_is_logged_cut_to_200_chars() {
        let (app, _dir) = test_app();
        // The owner's own chat turn and an MCP client are agents, but not the overseer.
        for source in ["chat", "mcp"] {
            let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
            answer(&app, &id, "stop", Some(source)).await.unwrap();
        }
        assert!(overseer::read(&app.data_dir, None, None).is_empty());
        let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
        let long = format!("stop {}", "x".repeat(400));
        answer(&app, &id, &long, Some("overseer")).await.unwrap();
        let log = overseer::read(&app.data_dir, None, None);
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!(log[0].kind, "answered");
        assert_eq!(log[0].task_id.as_deref(), Some(id.as_str()));
        assert_eq!(log[0].text.chars().count(), 200);
        assert!(
            log[0].text.starts_with("Plan question \u{2192} stop xxx"),
            "{}",
            log[0].text
        );
        let detail = log[0].detail.as_deref().unwrap();
        assert!(detail.contains("Allow it?") && detail.ends_with(&"x".repeat(400)));

        let id = waiting(&app, QuestionKind::PlanQuestion, &[]);
        answer(&app, &id, "stop", None).await.unwrap();
        assert_eq!(overseer::read(&app.data_dir, None, None).len(), 1);
    }

    #[tokio::test]
    async fn an_app_allow_with_no_source_is_accepted() {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::Permission, &["Allow once", "Deny"]);
        let rx = park_permission(&app, &id);
        answer(&app, &id, "Allow once", None).await.unwrap();
        assert_eq!(rx.await.unwrap().text, "Allow once");
    }

    #[tokio::test]
    async fn a_normal_question_is_still_answerable_through_mcp() {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
        let (tx, rx) = oneshot::channel();
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(Some(tx))),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        answer(&app, &id, "use the second design", Some("mcp"))
            .await
            .unwrap();
        assert_eq!(rx.await.unwrap().text, "use the second design");
    }

    async fn recorded_caller(source: Option<&str>, caller: &str, line: &str) {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::AgentQuestion, &["use B"]);
        answer(&app, &id, "use B", source).await.unwrap();
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert!(
            task.decisions.iter().any(|d| d == line),
            "{:?}",
            task.decisions
        );
        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(value["questionHistory"][0]["answeredBy"], caller);
        assert_eq!(value["questionHistory"][0]["answer"], "use B");
        assert_eq!(task.question_history.len(), 1);
        if caller != "owner" {
            assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        }
        app.shutdown();
    }

    #[tokio::test]
    async fn a_lead_answer_records_a_lead_line_and_history_without_owner_lines() {
        recorded_caller(Some("lead"), "lead", "Lead: use B").await;
    }

    #[tokio::test]
    async fn an_undeclared_answer_stays_the_owners() {
        recorded_caller(None, "owner", "Owner: use B").await;
        recorded_caller(Some(""), "owner", "Owner: use B").await;
    }

    #[tokio::test]
    async fn an_overseer_answer_records_the_overseer() {
        recorded_caller(
            Some("overseer"),
            "overseer",
            "Orchestrator: Allow it? -> use B (answered by the overseer)",
        )
        .await;
    }

    #[tokio::test]
    async fn a_chat_answer_keeps_the_orchestrator_line_and_history() {
        recorded_caller(
            Some("chat"),
            "orchestrator",
            "Orchestrator: Allow it? -> use B (answered by the orchestrator chat)",
        )
        .await;
    }

    #[tokio::test]
    async fn a_policy_answer_keeps_its_evidence_and_policy_history() {
        recorded_caller(
            Some("policy"),
            "policy",
            "Policy: Allow it? -> use B (policy: waiting SLA elapsed)",
        )
        .await;
    }

    #[tokio::test]
    async fn a_tasks_mcp_answer_is_recorded_as_a_lead() {
        recorded_caller(Some("mcp"), "lead", "Lead: use B").await;
    }

    #[tokio::test]
    async fn an_unknown_caller_and_a_policy_without_evidence_are_rejected() {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
        for source in ["owner", "judge", "other", " "] {
            let err = answer(&app, &id, "use B", Some(source)).await.unwrap_err();
            assert!(err.contains("unknown answer source"), "{err}");
        }
        let err = app
            .handle_task_answer(json!({"id": id, "answer": "use B", "source": "policy"}))
            .await
            .unwrap_err();
        assert!(err.contains("needs its evidence"), "{err}");
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Waiting);
        assert!(task.question.is_some());
        assert!(task.question_history.is_empty());
        assert!(task.decisions.is_empty());
    }

    #[tokio::test]
    async fn a_lead_stop_records_a_lead_stop_and_closes_the_task() {
        let (app, _dir) = test_app();
        let id = waiting(&app, QuestionKind::AgentQuestion, &[]);
        answer(&app, &id, " STOP ", Some("lead")).await.unwrap();
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Stopped);
        assert!(task.question.is_none());
        assert_eq!(task.decisions, ["Lead: stop (RPC)"]);
        assert_eq!(task.question_history[0].answered_by, AnsweredBy::Lead);
    }

    #[tokio::test]
    async fn a_lead_base_check_answer_names_the_lead() {
        let (app, _dir) = test_app();
        let id = waiting_base_check(&app, "npm run ci", None);
        answer(&app, &id, PRE_EXISTING_RETRY, Some("lead"))
            .await
            .unwrap();
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert!(task.decisions.contains(&format!(
            "Lead: base check npm run ci on deadbee -> {PRE_EXISTING_RETRY}"
        )));
        assert_eq!(answerer_label(&task), Some("Lead"));
        assert!(base_check_answered(&task, "npm run ci", &task.base_sha));
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        app.shutdown();
    }

    #[tokio::test]
    async fn lead_acceptance_is_refused_and_mixed_non_owner_continues_hit_the_cap() {
        let (app, _dir) = test_app();
        for callers in [
            [AnsweredBy::Lead, AnsweredBy::Overseer],
            [AnsweredBy::Orchestrator, AnsweredBy::Lead],
            [AnsweredBy::Overseer, AnsweredBy::Orchestrator],
        ] {
            let id = waiting(
                &app,
                QuestionKind::AttemptsFailing,
                &["continue", brief::ACCEPT_LAST_ATTEMPT, "stop"],
            );
            let err = answer(&app, &id, brief::ACCEPT_LAST_ATTEMPT, Some("lead"))
                .await
                .unwrap_err();
            assert!(err.contains("owner's to pick"), "{err}");
            let mut task = app.store.load_task(&id).unwrap().unwrap();
            let question = task.question.clone().unwrap();
            record_answer(&mut task, Some(&question), "continue", AnsweredBy::Owner);
            assert_eq!(non_owner_continues(&task), 0);
            for caller in callers {
                record_answer(&mut task, Some(&question), "continue", caller);
            }
            assert_eq!(non_owner_continues(&task), MAX_ORCHESTRATOR_CONTINUES);
            app.store.save_task(&task).unwrap();
            for source in ["lead", "overseer", "chat"] {
                let err = answer(&app, &id, "continue", Some(source))
                    .await
                    .unwrap_err();
                assert!(err.contains("the owner decides now"), "{err}");
            }
            let saved = app.store.load_task(&id).unwrap().unwrap();
            assert_eq!(saved.status, TaskStatus::Waiting);
            assert_eq!(
                saved.question.as_ref().unwrap().text,
                task.question.as_ref().unwrap().text
            );
            assert_eq!(saved.decisions, task.decisions);
            answer(&app, &id, "continue", None).await.unwrap();
            let saved = app.store.load_task(&id).unwrap().unwrap();
            assert_eq!(
                saved.question_history.last().unwrap().answered_by,
                AnsweredBy::Owner
            );
            let mut task = saved;
            task.question = Some(question);
            task.status = TaskStatus::Waiting;
            app.store.save_task(&task).unwrap();
            answer(&app, &id, "stop", Some("lead")).await.unwrap();
            assert_eq!(
                app.store.load_task(&id).unwrap().unwrap().status,
                TaskStatus::Stopped
            );
        }
        app.shutdown();
    }

    fn base_retry_task(commands: &[&str]) -> Task {
        let mut task = task_with_status(TaskStatus::Waiting);
        let failed: Vec<VerifyOutcome> = commands
            .iter()
            .map(|command| VerifyOutcome {
                command: (*command).into(),
                code: Some(1),
                tail: "fails".into(),
                ..Default::default()
            })
            .collect();
        task.question = Some(pre_existing_question(&failed, &task.base_sha));
        task
    }

    fn record_base_retry(task: &mut Task, commands: &[&str], caller: AnsweredBy) {
        let mut question = task.question.clone().unwrap();
        question.commands = commands.iter().map(|c| (*c).to_string()).collect();
        if matches!(caller, AnsweredBy::Policy | AnsweredBy::Judge) {
            let by = if caller == AnsweredBy::Policy {
                "policy"
            } else {
                "judge"
            };
            record_policy_answer(task, &question, PRE_EXISTING_RETRY, by, "fails on base", 1);
        } else {
            record_answer(task, Some(&question), PRE_EXISTING_RETRY, caller);
            let sha = task.base_sha.clone();
            record_owner_base_check(task, &question.commands, &sha, PRE_EXISTING_RETRY, false);
        }
    }

    #[tokio::test]
    async fn every_non_owner_retry_counts_on_the_same_base_but_the_owner_can_retry() {
        let (app, _dir) = test_app();
        for caller in [
            AnsweredBy::Policy,
            AnsweredBy::Judge,
            AnsweredBy::Orchestrator,
            AnsweredBy::Overseer,
            AnsweredBy::Lead,
        ] {
            for commands in [&["a.sh"][..], &["a.sh", "b.sh"][..]] {
                let mut task = base_retry_task(commands);
                record_base_retry(&mut task, &["a.sh"], caller);
                if commands.len() > 1 {
                    record_base_retry(&mut task, &["b.sh"], AnsweredBy::Lead);
                }
                app.store.save_task(&task).unwrap();
                for source in ["lead", "overseer", "chat", "mcp", "policy"] {
                    let err = answer(&app, &task.id, PRE_EXISTING_RETRY, Some(source))
                        .await
                        .unwrap_err();
                    assert!(err.contains("the owner decides now"), "{caller:?}: {err}");
                }
                let saved = app.store.load_task(&task.id).unwrap().unwrap();
                assert_eq!(saved.status, TaskStatus::Waiting);
                assert_eq!(saved.question_history, task.question_history);
                assert!(saved.question.is_some());
                answer(&app, &task.id, PRE_EXISTING_RETRY, None)
                    .await
                    .unwrap();
                let saved = app.store.load_task(&task.id).unwrap().unwrap();
                assert_eq!(
                    saved.question_history.last().unwrap().answered_by,
                    AnsweredBy::Owner
                );
            }
        }
        app.shutdown();
    }

    #[tokio::test]
    async fn an_uncovered_command_or_a_new_base_can_still_get_a_non_owner_retry() {
        let (app, _dir) = test_app();
        for caller in [
            AnsweredBy::Policy,
            AnsweredBy::Judge,
            AnsweredBy::Orchestrator,
            AnsweredBy::Overseer,
            AnsweredBy::Lead,
            AnsweredBy::Owner,
        ] {
            for changed_base in [false, true] {
                let mut task = base_retry_task(&["a.sh", "b.sh"]);
                record_base_retry(&mut task, &["a.sh"], caller);
                if changed_base {
                    record_base_retry(&mut task, &["b.sh"], caller);
                    task.base_sha = "1234567890".into();
                    task.question = Some(pre_existing_question(
                        &[
                            VerifyOutcome {
                                command: "a.sh".into(),
                                code: Some(1),
                                ..Default::default()
                            },
                            VerifyOutcome {
                                command: "b.sh".into(),
                                code: Some(1),
                                ..Default::default()
                            },
                        ],
                        &task.base_sha,
                    ));
                }
                app.store.save_task(&task).unwrap();
                answer(&app, &task.id, PRE_EXISTING_RETRY, Some("lead"))
                    .await
                    .unwrap();
                let saved = app.store.load_task(&task.id).unwrap().unwrap();
                assert_eq!(
                    saved.question_history.last().unwrap().answered_by,
                    AnsweredBy::Lead
                );
            }
        }
        app.shutdown();
    }

    #[tokio::test]
    async fn a_lead_dependency_answer_and_child_decision_name_the_lead() {
        let (app, _dir) = test_app();
        let mut parent = task_with_status(TaskStatus::Waiting);
        let mut child = task_with_status(TaskStatus::Failed);
        child.parent = Some(parent.id.clone());
        parent.question = Some(Question::new(
            format!("{DEPENDENCY_QUESTION}: child failed"),
            vec!["drop the dependency".into()],
            QuestionKind::DependencyEnded,
            AskedBy::Implement,
        ));
        app.store.save_task(&child).unwrap();
        app.store.save_task(&parent).unwrap();
        answer(&app, &parent.id, "drop the dependency", Some("lead"))
            .await
            .unwrap();
        let parent = app.store.load_task(&parent.id).unwrap().unwrap();
        let child = app.store.load_task(&child.id).unwrap().unwrap();
        assert_eq!(parent.question_history[0].answered_by, AnsweredBy::Lead);
        assert!(parent
            .decisions
            .iter()
            .any(|d| d == "Lead: drop the dependency"));
        assert!(parent.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert!(child
            .decisions
            .iter()
            .any(|d| d.starts_with("Lead: dropped from")));
        assert!(child.parent.is_none());
    }
}

#[cfg(test)]
mod stop_reason_tests {
    use super::*;

    #[test]
    fn a_stop_line_names_its_source_and_reason() {
        assert_eq!(
            AnswerSource::Owner.control_line("stop", None),
            "Owner: stop (UI)"
        );
        assert_eq!(
            AnswerSource::Owner.control_line("stop", Some(" wrong repo ")),
            "Owner: stop (UI): wrong repo"
        );
    }

    #[tokio::test]
    async fn stopping_an_idle_queued_task_records_who_and_why() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.queue.queue_reason = Some("waits for another".into());
        app.store.save_task(&task).unwrap();
        let out = app
            .handle_task_stop(json!({"id": task.id, "reason": "not needed"}))
            .await
            .unwrap();
        assert_eq!(out["status"], "stopped");
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(task.decisions, ["Owner: stop (UI): not needed"]);
        // Stopping it again adds nothing.
        app.handle_task_stop(json!({"id": task.id})).await.unwrap();
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(task.decisions.len(), 1);
    }
}
