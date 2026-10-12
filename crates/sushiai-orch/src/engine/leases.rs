use super::*;

/// How often a running task's worktree diff is read into its lease.
const LEASE_POLL: Duration = Duration::from_secs(2);

/// One task's claim on the files it has really changed in its repo and base
/// branch. A planned path is a hint for ordering, never a claim. Kept in
/// memory: a claim is only valid while the task's loop is live (or it waits
/// to land), so a restart starts with none and each loop reads its worktree
/// again at its gate.
#[derive(Default)]
pub(super) struct Lease {
    repo: String,
    base: String,
    /// Files the worktree diff shows, plus files `hook.edit` let the agent edit.
    files: Vec<String>,
}

impl Lease {
    fn add(&mut self, files: &[String]) {
        for file in files {
            if !self.files.contains(file) {
                self.files.push(file.clone());
            }
        }
    }
}

/// The task that holds a path someone else wants, and the path they share.
pub(super) struct Held {
    pub task_id: String,
    pub title: String,
    pub path: String,
}

impl Held {
    /// The wait of a task that has to wait for this holder.
    pub(super) fn wait(&self) -> Wait {
        Wait::File {
            task_id: self.task_id.clone(),
            title: self.title.clone(),
            path: self.path.clone(),
        }
    }

    /// The decision text of a task that has to wait.
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
    /// Terminal, parked and cancelled tasks release immediately. A task
    /// waiting to land keeps its claim without a loop.
    fn prune_leases(&self, leases: &mut HashMap<String, Lease>) {
        let controls: HashMap<String, bool> = self
            .controls
            .lock()
            .unwrap()
            .iter()
            .map(|(id, ctrl)| (id.clone(), !ctrl.cancel.is_cancelled()))
            .collect();
        leases.retain(|id, _| {
            self.store.load_task(id).ok().flatten().is_some_and(|t| {
                !t.archived
                    && !self.is_suspended(&t)
                    && !matches!(
                        t.status,
                        TaskStatus::Stopped | TaskStatus::Failed | TaskStatus::Done
                    )
                    && controls
                        .get(id)
                        .copied()
                        .unwrap_or(t.status == TaskStatus::Landing)
            })
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
            if let Some(path) = overlapping_path(wanted, &lease.files) {
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

    fn cancelled(&self, task: &Task) -> bool {
        self.controls
            .lock()
            .unwrap()
            .get(&task.id)
            .map_or(task.status == TaskStatus::Stopped, |ctrl| {
                ctrl.cancel.is_cancelled()
            })
    }

    /// Claims the files `task` has really changed (`changed`), or names the
    /// task in the way. With `force` (work already in the worktree, a landing
    /// to retry) the claim is made even where another task holds a file.
    pub(super) fn acquire_lease(
        &self,
        task: &Task,
        changed: &[String],
        force: bool,
    ) -> Option<Held> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        if self.cancelled(task) {
            return None;
        }
        let key = scope(task);
        if !force {
            if let Some(held) = self.conflict_in(&leases, &task.id, &key, changed) {
                return Some(held);
            }
        }
        let lease = leases.entry(task.id.clone()).or_default();
        lease.repo = key.0;
        lease.base = key.1;
        lease.add(changed);
        None
    }

    /// `hook.edit`: `Some` when another live task on the same base has
    /// really changed `file`; otherwise the file joins the caller's own lease.
    pub(super) fn lease_file(&self, task_id: &str, file: &str) -> Option<Held> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        let task = self.store.load_task(task_id).ok().flatten()?;
        if self.cancelled(&task) {
            return None;
        }
        let key = scope(&task);
        let wanted = [file.to_string()];
        if let Some(held) = self.conflict_in(&leases, task_id, &key, &wanted) {
            return Some(held);
        }
        let lease = leases.entry(task_id.to_string()).or_default();
        lease.repo = key.0;
        lease.base = key.1;
        lease.add(&wanted);
        None
    }

    /// Adds files the worktree diff of a live loop shows to its lease. No
    /// conflict check: the files are already changed.
    pub(super) fn sync_lease(&self, task_id: &str, files: &[String]) {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        if !leases.contains_key(task_id) && !self.controls.lock().unwrap().contains_key(task_id) {
            return;
        }
        let Some(task) = self.store.load_task(task_id).ok().flatten() else {
            return;
        };
        if self.cancelled(&task) {
            return;
        }
        let key = scope(&task);
        let lease = leases.entry(task_id.to_string()).or_default();
        lease.repo = key.0;
        lease.base = key.1;
        lease.add(files);
    }

    pub(super) fn leased_paths(&self, ids: &[String]) -> Vec<String> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        ids.iter()
            .filter_map(|id| leases.get(id))
            .flat_map(|lease| lease.files.clone())
            .collect()
    }

    /// `true` while the holder named by `task`'s `file` wait still holds that
    /// file.
    pub(super) fn file_blocked(&self, task: &Task) -> bool {
        let Some(Wait::File { task_id, path, .. }) = &task.queue.wait else {
            return false;
        };
        let mut leases = self.leases.lock().unwrap();
        self.prune_leases(&mut leases);
        leases
            .get(task_id)
            .is_some_and(|lease| lease.files.contains(path))
    }

    /// Ends the `file` waits whose holder landed or ended, and starts the
    /// tasks that were parked on one. Called whenever a loop ends or a task
    /// stops, the moments a lease can lapse.
    pub(super) fn advance_queue(&self, repo: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        for task in self.repo_tasks(repo) {
            if !matches!(task.queue.wait, Some(Wait::File { .. })) {
                continue;
            }
            let ended = task.archived
                || matches!(
                    task.status,
                    TaskStatus::Stopped | TaskStatus::Failed | TaskStatus::Done
                );
            // A task parked on a question no longer waits for a file.
            let asking = task.status == TaskStatus::Waiting;
            if !ended && !asking && self.file_blocked(&task) {
                continue;
            }
            if let Err(e) = self.update_task(&task.id, |t| t.queue.wait = None) {
                eprintln!("orch: updating task {} failed: {e}", task.id);
            }
            if ended
                || task.status != TaskStatus::Queued
                || task.queue.backlog.is_some()
                || self.is_suspended(&task)
                || self.controls.lock().unwrap().contains_key(&task.id)
            {
                continue;
            }
            self.spawn_task_loop(task.id.clone(), true);
        }
    }

    /// Reads the worktree diff of every other live task on the caller's base
    /// into its lease, so a file a shell command wrote is held before the
    /// caller's edit is judged.
    async fn read_other_worktrees(&self, task_id: &str) {
        let Some(task) = self.store.load_task(task_id).ok().flatten() else {
            return;
        };
        let key = scope(&task);
        let ids: Vec<String> = {
            let mut leases = self.leases.lock().unwrap();
            self.prune_leases(&mut leases);
            leases
                .iter()
                .filter(|(id, l)| id.as_str() != task_id && (&l.repo, &l.base) == (&key.0, &key.1))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in ids {
            let Some(other) = self.store.load_task(&id).ok().flatten() else {
                continue;
            };
            let (worktree, base) = (
                PathBuf::from(&other.worktree),
                other.work_base().to_string(),
            );
            let files = tokio::task::spawn_blocking(move || {
                git::changed_files(&worktree, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            self.sync_lease(&id, &files);
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
        // Only a file edit claims a path: an MCP tool's `path` argument is
        // not a file of this worktree.
        let edits = matches!(
            p.payload.get("tool_name").and_then(|t| t.as_str()),
            Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
        );
        let file = ["file_path", "notebook_path", "path"]
            .iter()
            .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()))
            .filter(|_| edits);
        if let Some(rel) = file.and_then(|f| relative_to_worktree(&ctx.worktree, f)) {
            self.read_other_worktrees(&ctx.task_id).await;
            if let Some(held) = self.lease_file(&ctx.task_id, &rel) {
                let reason = format!(
                    "{rel} is being edited by another task, \"{}\" ({}). Continue with other files; you get {rel} after that task lands.",
                    held.title, held.task_id
                );
                let line = format!(
                    "Queue: edit of {rel} refused, task \"{}\" holds {}",
                    held.title, held.path
                );
                self.append_lease_line(&ctx.task_id, line, held.wait());
                return Ok(json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                }));
            }
        }
        Ok(permissions::decide_call(&self.arc(), &ctx, &p.payload).await)
    }

    /// One decision line per distinct refusal; the task shows the `file` wait.
    fn append_lease_line(&self, task_id: &str, line: String, wait: Wait) {
        if let Err(e) = self.update_task(task_id, |task| {
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
            task.queue.wait = Some(wait);
        }) {
            eprintln!("orch: updating task {task_id} failed: {e}");
        }
    }
}

/// Keeps the lease of a live loop equal to its worktree diff: a poll while
/// the guard lives reads the changed files, also those a shell command wrote.
pub(super) struct LeaseWatch(tokio::task::JoinHandle<()>);

impl Drop for LeaseWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn watch_lease(app: &Arc<App>, task: &Task) -> LeaseWatch {
    let (app, id) = (app.clone(), task.id.clone());
    let (worktree, base) = (PathBuf::from(&task.worktree), task.work_base().to_string());
    LeaseWatch(tokio::spawn(async move {
        loop {
            tokio::time::sleep(LEASE_POLL).await;
            let (wt, b) = (worktree.clone(), base.clone());
            let files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &b).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            app.sync_lease(&id, &files);
        }
    }))
}

/// The lease gate of a task's loop, before it takes a slot. The lease holds
/// the files the worktree already changed. `true`: go on; `false`: the task
/// was parked `queued` with the file wait and the loop ends.
pub(super) async fn pass_lease_gate(app: &Arc<App>, task: &mut Task, force: bool) -> bool {
    let (worktree, base) = (PathBuf::from(&task.worktree), task.work_base().to_string());
    let changed = tokio::task::spawn_blocking(move || {
        git::changed_files(&worktree, &base).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    match app.acquire_lease(task, &changed, force) {
        None => {
            if matches!(task.queue.wait, Some(Wait::File { .. })) {
                task.queue.wait = None;
                task.decisions
                    .push("Queue: the files it changed are free, starting".to_string());
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
            }
            true
        }
        Some(held) => {
            let line = format!("Queue: {}", held.waits_line());
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
            task.queue.wait = Some(held.wait());
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn graph_control_stopped_holder_is_pruned_with_its_control_still_registered() {
        let (app, _dir) = test_app();
        let mut holder = task_with_status(TaskStatus::Running);
        holder.paths = vec!["shared.txt".into()];
        app.store.save_task(&holder).unwrap();
        let control = TaskControl {
            cancel: CancelToken::new(),
            pending_answer: Arc::new(StdMutex::new(None)),
            pending_amend: Arc::new(StdMutex::new(None)),
            handle: tokio::spawn(std::future::pending()),
        };
        app.controls
            .lock()
            .unwrap()
            .insert(holder.id.clone(), control);
        assert!(app.acquire_lease(&holder, &holder.paths, false).is_none());
        let mut waiter = holder.clone();
        waiter.id = "waiter".into();
        assert!(app.acquire_lease(&waiter, &waiter.paths, false).is_some());
        app.handle_task_stop(json!({"id": holder.id, "source": "lead"}))
            .await
            .unwrap();
        assert!(app.controls.lock().unwrap().contains_key(&holder.id));
        assert!(app.acquire_lease(&waiter, &waiter.paths, false).is_none());
        // A save of the winding loop's old copy cannot reclaim the paths.
        app.store.save_task(&holder).unwrap();
        assert!(app.acquire_lease(&waiter, &waiter.paths, false).is_none());
        app.apply_cancellation(&mut holder);
        app.store.save_task(&holder).unwrap();
        app.controls
            .lock()
            .unwrap()
            .remove(&holder.id)
            .unwrap()
            .handle
            .abort();
    }

    #[tokio::test]
    async fn graph_control_waiting_live_holder_keeps_its_lease() {
        let (app, _dir) = test_app();
        let mut holder = task_with_status(TaskStatus::Waiting);
        holder.paths = vec!["shared.txt".into()];
        holder.question = Some(Question::new(
            "Which way?",
            vec![],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        ));
        app.store.save_task(&holder).unwrap();
        app.controls.lock().unwrap().insert(
            holder.id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(std::future::pending()),
            },
        );
        assert!(app.acquire_lease(&holder, &holder.paths, false).is_none());
        let mut waiter = holder.clone();
        waiter.id = "waiter".into();
        assert!(app.acquire_lease(&waiter, &waiter.paths, false).is_some());
        app.controls
            .lock()
            .unwrap()
            .remove(&holder.id)
            .unwrap()
            .handle
            .abort();
    }

    #[tokio::test]
    async fn a_lease_holds_only_the_files_really_changed() {
        let (app, _dir) = test_app();
        let mut holder = task_with_status(TaskStatus::Running);
        holder.paths = vec!["planned.txt".into()];
        app.store.save_task(&holder).unwrap();
        app.controls.lock().unwrap().insert(
            holder.id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(std::future::pending()),
            },
        );
        // The gate of a fresh task claims nothing: the plan is no claim.
        assert!(app.acquire_lease(&holder, &[], false).is_none());
        let mut other = holder.clone();
        other.id = "other".into();
        app.store.save_task(&other).unwrap();
        assert!(app
            .acquire_lease(&other, &["planned.txt".into()], false)
            .is_none());
        // A file the worktree diff shows is held from then on.
        app.sync_lease(&holder.id, &["real.txt".into()]);
        let held = app
            .acquire_lease(&other, &["real.txt".into()], false)
            .expect("real.txt is held");
        assert_eq!(
            (held.task_id.as_str(), held.path.as_str()),
            (holder.id.as_str(), "real.txt")
        );
        // The refused task shows the wait until the holder lapses.
        other.queue.wait = Some(held.wait());
        assert!(app.file_blocked(&other));
        app.controls
            .lock()
            .unwrap()
            .remove(&holder.id)
            .unwrap()
            .handle
            .abort();
        assert!(!app.file_blocked(&other));
    }

    #[tokio::test]
    async fn advance_queue_clears_a_file_wait_once_the_task_asks_a_question() {
        let (app, _dir) = test_app();
        let holder = task_with_status(TaskStatus::Running);
        app.store.save_task(&holder).unwrap();
        app.controls.lock().unwrap().insert(
            holder.id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(std::future::pending()),
            },
        );
        app.sync_lease(&holder.id, &["real.txt".into()]);
        let mut other = task_with_status(TaskStatus::Running);
        other.queue.wait = Some(Wait::File {
            task_id: holder.id.clone(),
            title: "Holder".into(),
            path: "real.txt".into(),
        });
        app.store.save_task(&other).unwrap();
        // Still running and still refused: the wait stays.
        app.advance_queue(&other.repo);
        assert!(app
            .store
            .load_task(&other.id)
            .unwrap()
            .unwrap()
            .queue
            .wait
            .is_some());
        other.status = TaskStatus::Waiting;
        other.question = Some(Question::new(
            "Which way?",
            vec![],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        ));
        app.store.save_task(&other).unwrap();
        app.advance_queue(&other.repo);
        assert!(app
            .store
            .load_task(&other.id)
            .unwrap()
            .unwrap()
            .queue
            .wait
            .is_none());
        app.controls
            .lock()
            .unwrap()
            .remove(&holder.id)
            .unwrap()
            .handle
            .abort();
    }
}
