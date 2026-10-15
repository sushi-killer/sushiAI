use crate::engine::state::App;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_one_verify_command;
use crate::engine::worktrees;
use crate::harness::cancel::CancelToken;
use crate::task::Baseline;
use crate::task::PlannedTest;
use crate::task::Task;
use crate::verify::output::classify_baseline;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A throwaway checkout next to the task's worktree. It must stay outside the
/// data dir, which the read-only runs may not read: that would include their
/// own working directory.
pub(super) fn scratch_path(task: &Task, name: &str) -> PathBuf {
    PathBuf::from(format!("{}-{name}", task.worktree))
}

/// The tests-first rule for a rewrite: on a clean checkout of the base with
/// `files` written, every check that gated the task before must still fail.
pub(super) async fn still_fails_on_base(
    app: &Arc<App>,
    task: &Task,
    files: &[PlannedTest],
    cancel: &CancelToken,
) -> Result<(), String> {
    let gated = gated_checks(task);
    if gated.is_empty() {
        return Err("no check fails on the base, so the rewrite cannot be validated".into());
    }
    let repo = PathBuf::from(&task.repo);
    let run_dir = app.store.task_dir(&task.id).join("runs/claim-base");
    let path = scratch_path(task, "claim-base");
    let (r, p, sha) = (repo.clone(), path.clone(), task.base_sha.clone());
    let made = tokio::task::spawn_blocking(move || {
        crate::git::worktree::remove_worktree(&r, &p);
        crate::git::worktree::add_detached_worktree(&r, &p, &sha)?;
        let _ = crate::git::bootstrap::bootstrap_worktree(&r, &p);
        Ok::<(), crate::git::GitError>(())
    })
    .await;
    if !matches!(made, Ok(Ok(()))) {
        return Err("could not check out the base".into());
    }
    let result = async {
        for f in files {
            let target = path.join(&f.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, &f.content).map_err(|e| e.to_string())?;
        }
        let sandbox = app.task_sandbox(&task.repo, &path, &run_dir);
        worktrees::setup_worktree(&repo, &path, sandbox.as_ref(), cancel).await?;
        let timeout = Duration::from_secs(app.settings.read().unwrap().verify_timeout_secs);
        for check in &gated {
            let out = run_one_verify_command(
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(app, Some(&task.id)),
                    &path,
                    sandbox.as_ref(),
                    &task.base_sha,
                    cancel,
                )
                .with_timeout(timeout),
                &check.run,
            )
            .await;
            if classify_baseline(&out) != Some(Baseline::Fail) {
                return Err(format!(
                    "the rewritten test no longer fails on the base: `{}` does not fail there",
                    check.run
                ));
            }
        }
        Ok(())
    }
    .await;
    let (r, p) = (repo, path);
    let _ =
        tokio::task::spawn_blocking(move || crate::git::worktree::remove_worktree(&r, &p)).await;
    result
}

/// The only gate a planner rewrite passes: it must change the file and the
/// checks that gated the task must still fail on the base. Returns the test
/// files with the rewrite in place.
pub(super) async fn accept_rewrite(
    app: &Arc<App>,
    task: &Task,
    idx: usize,
    new: String,
    cancel: &CancelToken,
) -> Result<Vec<PlannedTest>, String> {
    if new == task.planner_tests.files[idx].content {
        return Err("the planner returned the test unchanged".into());
    }
    let mut files = task.planner_tests.files.clone();
    files[idx].content = new;
    still_fails_on_base(app, task, &files, cancel).await?;
    Ok(files)
}
