use crate::clock::now_ms;
use crate::engine::graph::{carry::CarryTree, landing::Landing};
use crate::engine::state::App;
use crate::git::{branches::BranchMove, paths::short_sha, GitError};
use crate::policy::settings::AfterLand;
use crate::task::{graph::landing::LandingRedirect, Task};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const AFTER_LAND_TIMEOUT: Duration = Duration::from_secs(600);

pub(super) async fn move_branch(
    tree: &CarryTree<'_>,
    task: &Task,
    branch: &str,
) -> Result<(String, BranchMove), GitError> {
    let attempt_n = tree.index.map_or(0, |index| task.attempts[index].n);
    let (wt, repo, branch, title, id, base) = (
        tree.worktree.to_path_buf(),
        PathBuf::from(&task.repo),
        branch.to_string(),
        task.title.clone(),
        task.id.clone(),
        task.base_sha.clone(),
    );
    tokio::task::spawn_blocking(move || {
        crate::delivery::git::commit::commit(&wt, &title, &id, attempt_n)?;
        let sha = crate::git::history::head_sha(&wt)?;
        let moved = crate::git::branches::move_branch(&repo, &branch, &sha);
        if !matches!(moved, Ok(BranchMove::Done { .. })) {
            let _ = crate::delivery::git::carry::uncommit_to(&wt, &base);
        }
        moved.map(|movement| (sha, movement))
    })
    .await
    .unwrap_or_else(|error| Err(GitError(error.to_string())))
}

pub(super) async fn redirected(
    app: &Arc<App>,
    task: &Task,
    branch: &str,
    sha: &str,
    redirects: &[Task],
) {
    for redirected in redirects.iter().filter(|done| done.landed_sha.is_none()) {
        let Ok(Some(mut done)) = app.store.load_task(&redirected.id) else {
            continue;
        };
        done.record_base_ref(Some(branch.to_string()));
        done.record_landed(Some(sha.to_string()));
        done.stamp_landing(now_ms());
        done.record_unlanded(None);
        done.record_decision(format!(
            "Land: task {} ({}) landed this work at {}",
            task.title,
            task.id,
            short_sha(sha)
        ));
        crate::engine::reports::refresh_diff_stat(&mut done).await;
        app.write_report(&mut done).await;
        done.touch(now_ms());
        let _ = app.store.save_task(&done);
        app.broadcast_task(&done);
    }
}

pub(super) async fn contained(
    app: &Arc<App>,
    task: &mut Task,
    target: &LandingRedirect,
) -> Landing {
    let (repo, branch) = (PathBuf::from(&task.repo), target.branch.clone());
    let landed = tokio::task::spawn_blocking(move || {
        crate::git::history::resolve_commit(&repo, &branch).ok()
    })
    .await
    .unwrap_or(None);
    task.record_landed(landed);
    task.record_unlanded(None);
    if let Some(sha) = task.landed_sha.as_deref() {
        redirected(app, task, &target.branch, sha, &target.done_tasks).await;
    }
    crate::engine::reports::record_landing(task).await;
    Landing::Landed
}

/// Runs the `settings.afterLand` commands for the task's repo in its main
/// checkout. Each exit code is a decision line; a failure never un-lands.
pub(super) async fn after_land(task: &mut Task, checkout: &Path, entries: &[AfterLand]) {
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
        task.record_decision(line);
    }
}
