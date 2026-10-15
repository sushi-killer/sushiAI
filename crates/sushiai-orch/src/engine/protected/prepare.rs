use crate::clock::now_ms;
use crate::engine::state::App;
use crate::engine::verification::baseline::baseline_checks;
use crate::engine::verification::baseline::needs_baseline;
use crate::harness::cancel::CancelToken;
use crate::harness::environment::augmented_path;
use crate::task::PlannedTest;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::paths::MAX_TEST_BYTES;
use super::snapshot::protected_hashes;
/// Before the first attempt something is left to do: a check has no baseline,
/// or the planner's files are not formatted and hashed yet.
pub(in crate::engine) fn needs_prepare(task: &Task) -> bool {
    let t = &task.planner_tests;
    needs_baseline(task)
        || (t.protected.is_none() && (!t.files.is_empty() || !task.checks.is_empty()))
}

/// Writes the planner's files into `worktree`; their repo-relative paths.
pub(in crate::engine) fn write_files(
    worktree: &Path,
    files: &[PlannedTest],
) -> Result<Vec<String>, String> {
    let root = worktree.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for t in files {
        let target = root.join(&t.path);
        let parent = target.parent().unwrap_or(&root);
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let real = parent.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(&root) {
            return Err(format!("{} resolves outside the worktree", t.path));
        }
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{} is a symlink", t.path));
        }
        if std::fs::read(&target).ok().as_deref() != Some(t.content.as_bytes()) {
            std::fs::write(&target, &t.content).map_err(|e| e.to_string())?;
        }
        paths.push(t.path.clone());
    }
    Ok(paths)
}

/// How long the repo's format command may run over the planner's files.
const FORMAT_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs the repo's format command (read at `rev`) over `paths` in `worktree`.
/// `Ok(false)`: the repo declares none.
async fn run_repo_format(
    app: &App,
    task: &Task,
    rev: &str,
    paths: &[String],
    cancel: &CancelToken,
) -> Result<bool, String> {
    let worktree = std::path::PathBuf::from(&task.worktree);
    let worktree = worktree.as_path();
    let Some(command) = crate::worktree_env::format_command_at(Path::new(&task.repo), rev)? else {
        return Ok(false);
    };
    let run_dir = app.store.task_dir(&task.id).join("runs/format");
    let _ = std::fs::create_dir_all(&run_dir);
    let sandbox = app.task_sandbox(&task.repo, worktree, &run_dir);
    crate::worktree_env::WorktreeEnvironment::new(
        Path::new(&task.repo),
        worktree,
        &augmented_path(),
        FORMAT_TIMEOUT,
    )
    .format(
        &command,
        paths,
        |script| {
            crate::verify::command::build_verify_command(worktree, script, sandbox.as_ref(), "")
        },
        cancel.cancelled(),
    )
    .await
    .map(|()| true)
}

/// Runs the repo's declared format command (read at `rev`) over the written
/// planner files and stores what it left in `task.planner_tests.files`, so the
/// commit, the hashes and the task agree. One decision line, or none without a
/// command.
async fn format_planner_tests(
    app: &App,
    task: &mut Task,
    paths: &[String],
    rev: &str,
    cancel: &CancelToken,
) -> Option<String> {
    let worktree = std::path::PathBuf::from(&task.worktree);
    let worktree = worktree.as_path();
    if let Err(e) = run_repo_format(app, task, rev, paths, cancel).await {
        for t in &task.planner_tests.files {
            let _ = std::fs::write(worktree.join(&t.path), &t.content);
        }
        return Some(format_failed(&e));
    }
    let mut changed = 0;
    let mut files = task.planner_tests.files.clone();
    for t in &mut files {
        if let Ok(text) = std::fs::read_to_string(worktree.join(&t.path)) {
            if text != t.content && text.len() <= MAX_TEST_BYTES {
                t.content = text;
                changed += 1;
            }
        }
    }
    crate::task::checks::edits::replace_planner_files(task, files);
    (changed > 0).then(|| {
        format!(
            "Orchestrator: formatted {changed} planner test file(s) with the repo's format command"
        )
    })
}

fn format_failed(reason: &str) -> String {
    let first = reason.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let short: String = first.chars().take(120).collect();
    format!("Orchestrator: format command failed on planner tests ({short}), stored as written")
}

/// The steps before attempt 1, each idempotent: write the planner's tests,
/// format them with the repo's format command and store what it left, baseline
/// the checks with the tests in place, store a sha256 per protected file.
/// `false` when cancelled.
pub(in crate::engine) async fn prepare_tests(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> bool {
    if !task.planner_tests.files.is_empty() && task.planner_tests.protected.is_none() {
        let (wt, files) = (worktree.to_path_buf(), task.planner_tests.files.clone());
        let written = tokio::task::spawn_blocking(move || write_files(&wt, &files))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        match written {
            Ok(paths) => {
                let rev = task.base_sha.clone();
                if let Some(line) = format_planner_tests(app, task, &paths, &rev, cancel).await {
                    task.record_decision(line);
                }
                task.record_decision(format!(
                    "Orchestrator: wrote {} planner test file(s); they are kept with the task and put back before every check",
                    paths.len()
                ));
            }
            Err(e) => {
                task.record_decision(format!(
                    "Orchestrator: planner tests not written, dropped: {e}"
                ));
                crate::task::checks::edits::replace_planner_files(task, Vec::new());
            }
        }
        task.touch(now_ms());
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
    }
    if !baseline_checks(app, task_id, task, worktree, cancel).await {
        return false;
    }
    if task.planner_tests.protected.is_none()
        && (!task.planner_tests.files.is_empty() || !task.checks.is_empty())
    {
        let rev = task.base_sha.clone();
        let (wt, snapshot) = (worktree.to_path_buf(), task.clone());
        let hashes = tokio::task::spawn_blocking(move || protected_hashes(&wt, &rev, &snapshot))
            .await
            .unwrap_or_default();
        task.record_protected_tests(hashes);
    }
    task.touch(now_ms());
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    true
}
