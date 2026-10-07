use super::*;

/// Worktree lifecycle: a task's worktree is removed once the task no longer
/// needs it (done, archived, deleted, `worktrees.gc`) and recreated from its
/// branch when the task runs again.
fn has_worktree(path: &str) -> bool {
    Path::new(path).join(".git").exists()
}

/// An archived task's saved evidence images are removed once it has not
/// changed for this long.
const EVIDENCE_KEEP_MS: i64 = 14 * 24 * 60 * 60 * 1000;

impl App {
    /// Removes the task's worktree if the task is done, or stopped/failed
    /// (its uncommitted work is first saved to the wip ref). Records
    /// `worktreeRemoved` and a decision line on `task`; the caller saves it.
    /// Returns the bytes freed, `None` when nothing was removed.
    pub(super) async fn release_worktree(&self, task: &mut Task, why: &str) -> Option<u64> {
        if !has_worktree(&task.worktree) {
            return None;
        }
        if !matches!(
            task.status,
            TaskStatus::Done | TaskStatus::Stopped | TaskStatus::Failed
        ) {
            return None;
        }
        self.collect_deliverables(task).await;
        let (repo, wt, id, save) = (
            task.repo.clone(),
            task.worktree.clone(),
            task.id.clone(),
            task.status != TaskStatus::Done,
        );
        let removed = tokio::task::spawn_blocking(move || {
            let mut saved = false;
            if save {
                saved = git::save_wip(Path::new(&wt), &id).map_err(|e| e.to_string())?;
            }
            let bytes = git::dir_size(Path::new(&wt));
            git::remove_task_worktree(Path::new(&repo), Path::new(&wt));
            Ok::<_, String>((bytes, saved))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        match removed {
            Ok((bytes, saved)) => {
                task.worktree_removed = true;
                task.decisions.push(format!(
                    "Worktree: removed {} ({why}{}; the work is on {})",
                    task.worktree,
                    if saved {
                        ", uncommitted work saved to the wip ref"
                    } else {
                        ""
                    },
                    task.branch
                ));
                Some(bytes)
            }
            Err(e) => {
                task.decisions
                    .push(format!("Worktree: kept, could not save its work ({e})"));
                None
            }
        }
    }

    /// Recreates the task's worktree from its branch when it is gone,
    /// restoring the work saved to the wip ref. `true` when it recreated one.
    pub(super) async fn ensure_worktree(&self, task: &mut Task) -> Result<bool, String> {
        if has_worktree(&task.worktree) {
            task.worktree_removed = false;
            return Ok(false);
        }
        let (repo, wt, branch, id) = (
            task.repo.clone(),
            task.worktree.clone(),
            task.branch.clone(),
            task.id.clone(),
        );
        let restored = tokio::task::spawn_blocking(move || {
            git::restore_worktree(Path::new(&repo), Path::new(&wt), &branch, &id)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not recreate the worktree of {}: {e}", task.branch))?;
        task.worktree_removed = false;
        task.decisions.push(format!(
            "Worktree: recreated {} from {}{}",
            task.worktree,
            task.branch,
            if restored { " with its saved work" } else { "" }
        ));
        Ok(true)
    }

    /// `worktrees.gc {dryRun}`: the cleanup rules applied to every task of
    /// every repo. A worktree with no task record is only listed.
    pub(super) async fn handle_worktrees_gc(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let dry_run = params
            .get("dryRun")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut tasks = self.store.list_tasks().map_err(|e| e.to_string())?;
        tasks.sort_by_key(|t| (t.created_at, t.id.clone()));
        let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
        let mut entries = Vec::new();
        let mut freed = 0u64;
        let mut repos: Vec<String> = Vec::new();
        let mut known = std::collections::HashSet::new();
        for task in &tasks {
            if !repos.contains(&task.repo) {
                repos.push(task.repo.clone());
            }
            known.insert(canon(&task.worktree));
        }
        let mut evidence_entries = Vec::new();
        for mut task in tasks {
            let live = self.controls.lock().unwrap().contains_key(&task.id);
            if !live && task.archived && now_ms() - task.updated_at > EVIDENCE_KEEP_MS {
                let (pruned, bytes) = self.prune_evidence(&mut task, dry_run).await;
                freed += bytes;
                evidence_entries.extend(pruned);
            }
            if !has_worktree(&task.worktree) {
                continue;
            }
            let (path, id) = (task.worktree.clone(), task.id.clone());
            let entry = move |action: &str, bytes: u64| json!({"path": path, "task": id, "action": action, "bytes": bytes});
            let live = self.controls.lock().unwrap().contains_key(&task.id);
            let wanted = !live
                && match task.status {
                    TaskStatus::Done => true,
                    TaskStatus::Stopped | TaskStatus::Failed => task.archived,
                    _ => false,
                };
            let size_path = task.worktree.clone();
            let bytes = tokio::task::spawn_blocking(move || git::dir_size(Path::new(&size_path)))
                .await
                .unwrap_or(0);
            if !wanted {
                entries.push(entry("kept", 0));
                continue;
            }
            if dry_run {
                freed += bytes;
                entries.push(entry("would-remove", bytes));
                continue;
            }
            match self.release_worktree(&mut task, "gc").await {
                Some(bytes) => {
                    freed += bytes;
                    task.updated_at = now_ms();
                    let _ = self.store.save_task(&task);
                    self.broadcast_task(&task);
                    entries.push(entry("removed", bytes));
                }
                None => entries.push(entry("kept", 0)),
            }
        }
        for repo in &repos {
            let repo_path = PathBuf::from(repo);
            let listed = tokio::task::spawn_blocking({
                let repo_path = repo_path.clone();
                move || git::linked_worktrees(&repo_path)
            })
            .await
            .unwrap_or_default();
            for path in listed {
                if known.contains(&canon(&path.to_string_lossy())) {
                    continue;
                }
                let p = path.clone();
                let bytes = tokio::task::spawn_blocking(move || git::dir_size(&p))
                    .await
                    .unwrap_or(0);
                entries.push(json!({
                    "path": path.to_string_lossy(), "task": null, "action": "unknown", "bytes": bytes
                }));
            }
            if !dry_run {
                let _ = tokio::task::spawn_blocking(move || git::prune_worktrees(&repo_path)).await;
            }
        }
        Ok(json!({
            "dryRun": dry_run,
            "worktrees": entries,
            "evidence": evidence_entries,
            "freedBytes": freed
        }))
    }

    /// Removes `tasks/<id>/runs/*/evidence` of an archived task and drops the
    /// vanished copies from its attempts. Returns the report entries and the
    /// bytes (a dry run changes nothing and reports `would-remove`).
    async fn prune_evidence(
        &self,
        task: &mut Task,
        dry_run: bool,
    ) -> (Vec<serde_json::Value>, u64) {
        let runs = self.store.task_dir(&task.id).join("runs");
        let dirs: Vec<PathBuf> = std::fs::read_dir(&runs)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path().join("evidence"))
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        let mut entries = Vec::new();
        let mut total = 0u64;
        for dir in dirs {
            let d = dir.clone();
            let bytes = tokio::task::spawn_blocking(move || git::dir_size(&d))
                .await
                .unwrap_or(0);
            if !dry_run && std::fs::remove_dir_all(&dir).is_err() {
                continue;
            }
            total += bytes;
            entries.push(json!({
                "task": task.id,
                "path": dir.to_string_lossy(),
                "action": if dry_run { "would-remove" } else { "removed" },
                "bytes": bytes
            }));
        }
        if !dry_run && !entries.is_empty() {
            for a in &mut task.attempts {
                a.evidence.retain(|p| Path::new(p).exists());
            }
            let _ = self.store.save_task(task);
        }
        (entries, total)
    }
}
