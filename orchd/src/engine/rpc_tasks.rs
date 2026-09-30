use super::*;

/// The most lines `task.log` returns: the last ones.
const TASK_LOG_MAX_LINES: usize = 10_000;

/// Everything `create_task_record` needs for a new task.
pub(super) struct NewTask {
    pub(super) id: String,
    pub(super) backlog: Option<Backlog>,
    pub(super) repo_root: PathBuf,
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
        // A plan attempt and the first implement attempt can share a number:
        // the stage tells them apart.
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
        let (path, harness) = match p.stage.as_str() {
            "plan" => (run_dir.join("plan").join("events.jsonl"), attempt.harness),
            "implement" => (run_dir.join("events.jsonl"), attempt.harness),
            "review" => (
                run_dir.join("review").join("events.jsonl"),
                fingerprint_harness(&attempt.review_fingerprint),
            ),
            _ => (
                run_dir.join("advisor").join("events.jsonl"),
                fingerprint_harness(&attempt.advisor_fingerprint),
            ),
        };
        let (lines, truncated) = tokio::task::spawn_blocking(move || {
            let bytes =
                std::fs::read(&path).map_err(|_| "no stored log for this stage".to_string())?;
            let text = String::from_utf8_lossy(&bytes);
            let mut outcome = harness::RunOutcome::default();
            let mut lines: Vec<String> = text
                .lines()
                .filter_map(|l| harness::feed_stream_line(harness, l, &mut outcome))
                .collect();
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
        let mut variant = resolve_variant(&settings.experiments, p.variant.as_ref())?;
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
            self.check_graph_params(&repo, &p.depends_on, p.parent.as_deref())?;
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
                relay_of: None,
                eval_set: p.eval_set.clone().filter(|s| !s.trim().is_empty()),
                eval_name: p.eval_name.clone().filter(|s| !s.trim().is_empty()),
                eval_check_cmd: p.eval_check.clone().filter(|s| !s.trim().is_empty()),
                source: Some(source),
                created_at: now_ms(),
            })
            .await?;
        if let Some(cmd) = p.screenshot.as_ref().filter(|c| !c.trim().is_empty()) {
            task.brief_check.screenshot = Some(cmd.trim().to_string());
            self.store.save_task(&task).map_err(|e| e.to_string())?;
        }
        let id = task.id.clone();
        if let Some(mcp) = &p.mcp {
            let path = self.store.task_dir(&id).join("mcp.json");
            store::write_json_atomic(&path, mcp).map_err(|e| e.to_string())?;
        }
        self.broadcast_task(&task);
        if request_text.is_some() {
            // Planning always runs regardless of `start`; only whether the
            // loop falls through into implementing once it's done depends
            // on it (spec step 6).
            self.spawn_task_loop(id.clone(), p.start.unwrap_or(true) && backlog.is_none());
        } else if backlog.is_none() && (p.start.unwrap_or(true) || in_graph) {
            // Same default as the request form: a `queued` task that never
            // starts until a daemon restart looks like work in progress. A
            // task in a graph starts on its own: its loop waits for what it
            // depends on.
            self.start_task_loop(id.clone());
        }
        if backlog.is_some() {
            self.advance_autopilot();
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// `task.create`'s `dependsOn`/`parent`: every id a task of this repo,
    /// the parent a top-level task that has not implemented anything itself
    /// and is not done, and no dependency cycle once the new task is added
    /// (a parent waits for its children, so a child depending on its own
    /// parent is one). Returns the deduplicated dependencies and the parent.
    fn check_graph_params(
        &self,
        repo: &str,
        depends_on: &[String],
        parent: Option<&str>,
    ) -> Result<(Vec<String>, Option<Task>), String> {
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
        let mut deps: Vec<String> = Vec::new();
        for id in depends_on {
            load(id, "dependency")?;
            if !deps.contains(id) {
                deps.push(id.clone());
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
                if parent.status != TaskStatus::Drafting
                    && self.controls.lock().unwrap().contains_key(&parent.id)
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
        let mut edges = wait_edges(&self.repo_tasks(repo));
        edges.insert(new_id.clone(), deps.clone());
        if let Some(parent) = &parent {
            edges.entry(parent.id.clone()).or_default().push(new_id);
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
        let created = tokio::task::spawn_blocking(move || {
            let branch = branch_opt
                .unwrap_or_else(|| git::unique_branch_name(&repo_root, &title_for_branch));
            let wt_path = git::worktree_path(&repo_root, &wt_root, &branch);
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
            status: if new.request.is_some() {
                TaskStatus::Drafting
            } else {
                TaskStatus::Queued
            },
            request: new.request,
            repo: new.repo_root.to_string_lossy().to_string(),
            worktree: created.path.to_string_lossy().to_string(),
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
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
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Some(new.variant),
            eval_set: new.eval_set,
            eval_name: new.eval_name,
            eval_check_cmd: new.eval_check_cmd,
            source: new.source,
            eval_check: None,
            brief_check: Default::default(),
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
        let already_running = self.controls.lock().unwrap().contains_key(&p.id);
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
            ) {
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
        let mut task = self
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
            return Err(
                "the task was not started from a branch, so there is nothing to land on"
                    .to_string(),
            );
        };
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
                return Err(crate::model::land_on_default_refusal(&branch, &task.repo));
            }
        }
        let all = self.repo_tasks(&task.repo);
        if task.attempts.is_empty() && !is_parent(&task, &all) {
            return Err("the task has no finished attempt to land".to_string());
        }
        let mut variant = task.variant();
        variant.land = true;
        task.variant = Some(variant);
        task.status = TaskStatus::Landing;
        task.decisions
            .push(format!("Land: requested, landing on {branch}"));
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
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
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let live = match self.controls.lock().unwrap().get(&p.id) {
            Some(ctrl) => {
                ctrl.cancel.cancel();
                true
            }
            None => false,
        };
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        // A task in a graph can sit idle with no loop to cancel: a parent
        // while its children work, a task waiting for its dependencies.
        let all = self.repo_tasks(&task.repo);
        let parent = is_parent(&task, &all);
        if parent {
            self.stop_children(&task.id, &task.repo);
        }
        if !live && task.status == TaskStatus::Landing {
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.release_worktree(&mut task, "stopped while waiting to land")
                .await;
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            // Its lease lapses: whoever waited for it can go.
            self.advance_graph(&task.repo);
        }
        if !live
            && (parent || !task.depends_on.is_empty() || task.queue.queue_reason.is_some())
            && matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting
            )
        {
            task.question = None;
            task.queue.queue_reason = None;
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.advance_graph(&task.repo);
        }
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
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let assumption = task
            .assumptions
            .get_mut(p.index)
            .ok_or_else(|| format!("task has no assumption {}", p.index))?;
        assumption.overturned = true;
        assumption.owner_answer = Some(answer.to_string());
        let text = format!(
            "The owner overturned an assumption made for them ({}). Question: {}\nThe assumption: {}\nThe owner's answer: {answer}\nUse the owner's answer.",
            assumption.by, assumption.question, assumption.answer
        );
        if task.status != TaskStatus::Done {
            messages::send(self, ORCHESTRATOR, Some(&task.id), &text, None)?;
        }
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
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
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Waiting || task.question.is_none() {
            return Err("task is not waiting for an answer".to_string());
        }

        if p.answer.trim().eq_ignore_ascii_case("stop") {
            if let Some(ctrl) = self.controls.lock().unwrap().get(&p.id) {
                ctrl.cancel.cancel();
            }
            task.decisions.push("Owner: stop".to_string());
            if let Some(question) = task.question.take() {
                record_answered_question(&mut task, &question, "stop", AnsweredBy::Owner);
            }
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.stop_children(&task.id, &task.repo);
            self.advance_graph(&task.repo);
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }

        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
        {
            return self.answer_dependency_question(task, &p.answer, true);
        }

        // A tool call held for this question: the run goes on with the answer.
        if let Some(tx) = self.permission_waits.lock().unwrap().remove(&p.id) {
            if tx.send(p.answer.clone()).is_ok() {
                return serde_json::to_value(&task).map_err(|e| e.to_string());
            }
        }

        let delivered = {
            let controls = self.controls.lock().unwrap();
            controls
                .get(&p.id)
                .and_then(|c| c.pending_answer.lock().unwrap().take())
                .map(|tx| tx.send(p.answer.clone()).is_ok())
                .unwrap_or(false)
        };
        if !delivered {
            // No live parked loop (e.g. after a daemon restart): apply the
            // decision synchronously and relaunch (spec step 9: "status
            // queued, loop continues").
            task.decisions.push(format!("Owner: {}", p.answer));
            if let Some(question) = task.question.clone() {
                record_answered_question(&mut task, &question, &p.answer, AnsweredBy::Owner);
            }
            if let Some(n) = task
                .question
                .as_ref()
                .and_then(|q| impossible_drop_target(q, &p.answer))
            {
                drop_criterion(&mut task, n);
            }
            if let Some(q) = task.question.clone() {
                // A base-check question: the task starts again only with the
                // answer applied, and the owner's answer is remembered.
                if let Some(command) = pre_existing_command(&q).map(str::to_string) {
                    let base_sha = task.base_sha.clone();
                    record_owner_base_check(&mut task, &command, &base_sha, &p.answer, false);
                    apply_base_check_answer(&mut task, &q, &p.answer);
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
                    record_owner_accept(
                        &mut task,
                        &q.text,
                        &p.answer,
                        "owner picked the option",
                        n,
                    );
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
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.start_task_loop(p.id.clone());
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// Owner amendment of criteria/verify/finalVerify/checks/heldOut. A task
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
        let amendment = Amendment {
            criteria,
            visual_criteria,
            verify: field(&params, "verify", "an array of strings")?,
            final_verify: field(&params, "finalVerify", "an array of strings")?,
            screenshot: field(&params, "screenshot", "a string")?,
            checks: field(&params, "checks", "an array of {criterion, run} objects")?,
            held_out,
        };
        if amendment.is_empty() {
            return Err(
                "task.amend needs at least one of criteria, verify, finalVerify, screenshot, checks, heldOut"
                    .to_string(),
            );
        }
        validate_task_id(&self.store, &id)?;
        let mut task = self
            .store
            .load_task(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.archived {
            return Err("task is archived".to_string());
        }
        if !matches!(
            task.status,
            TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Queued | TaskStatus::Stopped
        ) {
            return Err(format!(
                "task is {:?}: only running, waiting, queued or stopped tasks can be amended",
                task.status
            )
            .to_lowercase());
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
        amendment.apply(&mut task);
        if task.variant().grounded_checks {
            baseline_on_base(&self.arc(), &mut task, &CancelToken::new()).await;
        }
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        Ok(json!({"id": id, "amended": fields, "pending": false}))
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
        task.archived = true;
        self.release_worktree(&mut task, "archived").await;
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
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
        task.archived = false;
        if matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed) {
            if let Err(e) = self.ensure_worktree(&mut task).await {
                task.decisions.push(format!("Worktree: {e}"));
            }
        }
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
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
