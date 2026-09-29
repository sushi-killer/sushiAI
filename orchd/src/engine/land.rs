use super::*;

/// How often a task waiting `landing` is tried again.
const LANDING_RETRY: Duration = Duration::from_secs(120);
const AFTER_LAND_TIMEOUT: Duration = Duration::from_secs(600);

/// Lands a finished top-level task with `variant.land` on its base branch,
/// one task per (repo, branch) at a time: its work is carried onto the
/// branch's current head, squashed to one commit, checked again on exactly
/// that tree when the head had moved, and the branch moves to the commit.
/// `Landing::NoParent` means landing was refused: the task commits on its own
/// branch instead.
pub(super) async fn land_on_base(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Landing {
    let (land_on_default, after_land) = {
        let s = app.settings.read().unwrap();
        (s.land_on_default, s.after_land.clone())
    };
    let Some(branch) = task.base_ref.clone() else {
        task.decisions.push(
            "Land: refused, the task was not started from a branch; the work stays on its own branch"
                .to_string(),
        );
        return Landing::NoParent;
    };
    let repo = PathBuf::from(&task.repo);
    if !land_on_default {
        let r = repo.clone();
        let default = tokio::task::spawn_blocking(move || git::default_branch(&r))
            .await
            .unwrap_or(None);
        if default.as_deref() == Some(branch.as_str()) {
            task.decisions.push(format!(
                "Land: refused, {branch} is the default branch and settings.landOnDefault is off; the work stays on {}",
                task.branch
            ));
            return Landing::NoParent;
        }
    }

    let lock = app.landing_lock(&format!("base\0{}\0{branch}", task.repo));
    let _landing = tokio::select! {
        _ = cancel.cancelled() => return Landing::Cancelled,
        guard = lock.lock_owned() => guard,
    };

    let finals = task.final_verify.clone();
    match carry_and_check(
        app,
        task,
        idx,
        worktree,
        run_dir,
        &branch,
        "Another task",
        &finals,
        cancel,
    )
    .await
    {
        Ok(true) => {
            let (r, b) = (repo.clone(), branch.clone());
            task.landed_sha = tokio::task::spawn_blocking(move || git::resolve_commit(&r, &b).ok())
                .await
                .unwrap_or(None);
            return Landing::Landed;
        }
        Ok(false) => {}
        Err(landing) => return landing,
    }

    let (wt, r, b, title, tid, base) = (
        worktree.to_path_buf(),
        repo.clone(),
        branch.clone(),
        task.title.clone(),
        task.id.clone(),
        task.base_sha.clone(),
    );
    let moved = tokio::task::spawn_blocking(move || {
        git::commit(&wt, &title, &tid, attempt_n)?;
        let sha = git::head_sha(&wt)?;
        let moved = git::move_branch(&r, &b, &sha);
        if !matches!(moved, Ok(git::BranchMove::Done { .. })) {
            // Keep the work, uncommitted, for the next try to carry.
            let _ = git::uncommit_to(&wt, &base);
        }
        moved.map(|m| (sha, m))
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match moved {
        Ok((sha, git::BranchMove::Done { checkout })) => {
            task.landed_sha = Some(sha.clone());
            task.decisions
                .push(format!("Land: landed on {branch} at {}", short_sha(&sha)));
            run_after_land(task, &checkout, &after_land).await;
            Landing::Landed
        }
        Ok((_, git::BranchMove::Dirty { checkout })) => {
            if task.status != TaskStatus::Landing {
                task.decisions.push(format!(
                    "Land: waiting, {} has uncommitted changes; retrying every 2 minutes and on task.start",
                    checkout.display()
                ));
            }
            Landing::Waiting
        }
        Ok((_, git::BranchMove::Diverged)) => Landing::Failed {
            kind: FailureKind::Error,
            detail: format!("{branch} moved while landing; carry the work onto it again."),
        },
        Err(e) => Landing::Failed {
            kind: FailureKind::Error,
            detail: format!("Could not land on {branch}: {e}"),
        },
    }
}

/// Runs the `settings.afterLand` commands for the task's repo in its main
/// checkout. Each exit code is a decision line; a failure never un-lands.
async fn run_after_land(task: &mut Task, checkout: &Path, entries: &[AfterLand]) {
    let same_repo = |entry: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(entry) == canon(Path::new(&task.repo))
    };
    let entries: Vec<&AfterLand> = entries
        .iter()
        .filter(|e| same_repo(Path::new(&e.repo)))
        .collect();
    for entry in entries {
        let run = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&entry.run)
            .current_dir(checkout)
            .kill_on_drop(true)
            .output();
        let line = match tokio::time::timeout(AFTER_LAND_TIMEOUT, run).await {
            Ok(Ok(out)) => {
                let code = out.status.code();
                let code = code.map_or_else(|| "a signal".to_string(), |c| c.to_string());
                let mut line = format!("After land: `{}` exited {code}", entry.run);
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let tail = err.trim();
                    let start = tail.len().saturating_sub(300);
                    let start = (start..=tail.len())
                        .find(|i| tail.is_char_boundary(*i))
                        .unwrap_or(tail.len());
                    if !tail[start..].is_empty() {
                        line.push_str(&format!(": {}", &tail[start..]));
                    }
                }
                line
            }
            Ok(Err(e)) => format!("After land: `{}` could not start ({e})", entry.run),
            Err(_) => format!(
                "After land: `{}` timed out after {} seconds",
                entry.run,
                AFTER_LAND_TIMEOUT.as_secs()
            ),
        };
        task.decisions.push(line);
    }
}

impl App {
    /// Starts the loop of every `landing` task that has none: called every
    /// [`LANDING_RETRY`], on daemon start and by `task.start`.
    pub(super) fn retry_landings(&self) {
        for t in self.store.list_tasks().unwrap_or_default() {
            if t.status == TaskStatus::Landing && !t.archived {
                self.spawn_task_loop(t.id, true);
            }
        }
    }

    /// The 2-minute retry of tasks waiting `landing`; ends with the daemon.
    pub(super) fn spawn_landing_retries(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(LANDING_RETRY);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                app.retry_landings();
            }
        });
    }
}
