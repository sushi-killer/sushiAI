use super::*;
use std::collections::HashSet;

/// One task's claim on files of its repo and base branch. Kept in memory:
/// a claim is only valid while the task's loop is live (or it waits to land),
/// so a restart starts with none and each loop claims again at its gate.
#[derive(Default)]
pub(super) struct Lease {
    repo: String,
    base: String,
    /// The paths the planner declared for the task.
    declared: Vec<String>,
    /// Files the agent edited that no other task held (`hook.edit`).
    files: Vec<String>,
}

impl Lease {
    fn held(&self) -> Vec<String> {
        self.declared.iter().chain(&self.files).cloned().collect()
    }
}

/// The task that holds a path someone else wants, and the path they share.
pub(super) struct Held {
    pub task_id: String,
    pub title: String,
    pub path: String,
}

impl Held {
    /// The queue reason and decision text of a task that has to wait.
    pub(super) fn waits_line(&self) -> String {
        format!("waits for \"{}\" on {}", self.title, self.path)
    }
}

/// The lease scope of a task: tasks conflict only on one repo and base branch.
fn scope(task: &Task) -> (String, String) {
    (task.repo.clone(), task.base_ref.clone().unwrap_or_default())
}

/// A file path as the leases compare it: repo-relative, no `./`.
pub(super) fn relative_to_worktree(worktree: &Path, file: &str) -> Option<String> {
    let path = Path::new(file);
    if !path.is_absolute() {
        let trimmed = file.trim_start_matches("./");
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    let mut roots = vec![worktree.to_path_buf()];
    if let Ok(real) = worktree.canonicalize() {
        roots.push(real);
    }
    let candidates = [
        path.to_path_buf(),
        path.parent()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.join(path.file_name().unwrap_or_default()))
            .unwrap_or_else(|| path.to_path_buf()),
    ];
    for root in roots {
        for candidate in &candidates {
            if let Ok(rel) = candidate.strip_prefix(&root) {
                let rel = rel.to_string_lossy().to_string();
                if !rel.is_empty() {
                    return Some(rel);
                }
            }
        }
    }
    None
}

impl App {
    /// Drops the claims of tasks whose loop ended: the task landed, stopped
    /// or failed. A task waiting to land keeps its claim without a loop.
    fn prune_leases(&self, leases: &mut HashMap<String, Lease>) {
        let live: HashSet<String> = self.controls.lock().unwrap().keys().cloned().collect();
        leases.retain(|id, _| {
            live.contains(id)
                || self
                    .store
                    .load_task(id)
                    .ok()
                    .flatten()
                    .is_some_and(|t| t.status == TaskStatus::Landing && !t.archived)
        });
    }

    fn conflict_in(
        &self,
        leases: &HashMap<String, Lease>,
        task_id: &str,
        (repo, base): &(String, String),
        wanted: &[String],
    ) -> Option<Held> {
        if wanted.is_empty() {
            return None;
        }
        let mut ids: Vec<&String> = leases.keys().collect();
        ids.sort();
        for id in ids {
            let lease = &leases[id];
            if id == task_id || &lease.repo != repo || &lease.base != base {
                continue;
            }
            if let Some(path) = overlapping_path(wanted, &lease.held()) {
                let title = self
                    .store
                    .load_task(id)
                    .ok()
                    .flatten()
                    .map(|t| t.title)
                    .unwrap_or_else(|| id.clone());
                return Some(Held {
                    task_id: id.clone(),
                    title,
                    path: path.to_string(),
                });
            }
        }
        None
    }

    /// The task that holds a path `task` declared, if any (no claim is made).
    pub(super) fn lease_blocker(&self, task: &Task) -> Option<Held> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        self.conflict_in(&leases, &task.id, &scope(task), &task.paths)
    }

    /// Claims the paths `task` declared, or names the task in the way. With
    /// `force` (work already in the worktree, a landing to retry) the claim is
    /// made even where another task holds the same path.
    pub(super) fn acquire_lease(&self, task: &Task, force: bool) -> Option<Held> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        let key = scope(task);
        if !force {
            if let Some(held) = self.conflict_in(&leases, &task.id, &key, &task.paths) {
                return Some(held);
            }
        }
        let lease = leases.entry(task.id.clone()).or_default();
        lease.repo = key.0;
        lease.base = key.1;
        lease.declared = task.paths.clone();
        None
    }

    /// `hook.edit`: `Some` when another live task on the same base holds
    /// `file`; otherwise the file joins the caller's own lease.
    pub(super) fn lease_file(&self, task_id: &str, file: &str) -> Option<Held> {
        let task = self.store.load_task(task_id).ok().flatten()?;
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        let key = scope(&task);
        let wanted = [file.to_string()];
        if let Some(held) = self.conflict_in(&leases, task_id, &key, &wanted) {
            return Some(held);
        }
        let lease = leases.entry(task_id.to_string()).or_insert_with(|| Lease {
            repo: key.0,
            base: key.1,
            declared: task.paths.clone(),
            files: vec![],
        });
        if !lease.files.iter().any(|f| f == file) {
            lease.files.push(file.to_string());
        }
        None
    }

    /// Starts the tasks that waited for a lease and are free now. Called
    /// whenever a loop ends or a task stops, the moments a lease can lapse.
    pub(super) fn advance_queue(&self, repo: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        for task in self.repo_tasks(repo) {
            if task.archived
                || task.status != TaskStatus::Queued
                || task.queue.queue_reason.is_none()
                || self.controls.lock().unwrap().contains_key(&task.id)
                || self.lease_blocker(&task).is_some()
            {
                continue;
            }
            self.spawn_task_loop(task.id.clone(), true);
        }
    }

    /// `hook.edit`: the PreToolUse decision for one edit of the calling run.
    pub(super) async fn handle_hook_edit(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            token: String,
            #[serde(default)]
            payload: serde_json::Value,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let ctx = { self.hook_tokens.read().unwrap().get(&p.token).cloned() };
        let Some(ctx) = ctx else {
            return Ok(json!({}));
        };
        let input = p.payload.get("tool_input");
        let file = ["file_path", "notebook_path", "path"]
            .iter()
            .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()));
        let Some(rel) = file.and_then(|f| relative_to_worktree(&ctx.worktree, f)) else {
            return Ok(json!({}));
        };
        match self.lease_file(&ctx.task_id, &rel) {
            None => Ok(json!({})),
            Some(held) => {
                let reason = format!(
                    "{rel} is being edited by another task, \"{}\" ({}). Continue with other files; you get {rel} after that task lands.",
                    held.title, held.task_id
                );
                let line = format!(
                    "Queue: edit of {rel} refused, task \"{}\" holds {}",
                    held.title, held.path
                );
                self.append_lease_line(&ctx.task_id, line);
                Ok(json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                }))
            }
        }
    }

    /// One decision line per distinct refusal.
    fn append_lease_line(&self, task_id: &str, line: String) {
        if let Ok(Some(mut task)) = self.store.load_task(task_id) {
            if task.decisions.contains(&line) {
                return;
            }
            task.decisions.push(line);
            task.updated_at = now_ms();
            let _ = self.store.save_task(&task);
            self.broadcast_task(&task);
        }
    }
}

/// The lease gate of a task's loop, before it takes a slot. `true`: go on;
/// `false`: the task was parked `queued` with the reason and the loop ends.
pub(super) fn pass_lease_gate(app: &Arc<App>, task: &mut Task, force: bool) -> bool {
    match app.acquire_lease(task, force) {
        None => {
            if task.queue.queue_reason.take().is_some() {
                task.decisions
                    .push("Queue: the paths it needs are free, starting".to_string());
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
            }
            true
        }
        Some(held) => {
            let reason = held.waits_line();
            let line = format!("Queue: {reason}");
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
            task.queue.queue_reason = Some(reason);
            task.queue.waited_on_lease = true;
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
            false
        }
    }
}
