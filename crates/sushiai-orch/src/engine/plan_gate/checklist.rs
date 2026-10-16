use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

/// Runs each checklist item's check in the worktree and records `passes`.
/// Returns the failure detail naming the failing items, or `None` when all
/// pass, there is nothing to run, or the run was cancelled.
pub(in crate::engine) async fn run(
    app: &Arc<App>,
    task: &mut Task,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<String> {
    let runnable: Vec<usize> = (0..task.checklist.len())
        .filter(|&i| !task.checklist[i].check.trim().is_empty())
        .collect();
    if runnable.is_empty() {
        return None;
    }
    let commands: Vec<String> = runnable
        .iter()
        .map(|&i| task.checklist[i].check.clone())
        .collect();
    let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
    let results = run_verify_commands(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(app, Some(&task.id)),
            worktree,
            sandbox.as_ref(),
            &task.base_sha,
            cancel,
        ),
        &commands,
    )
    .await;
    if cancel.is_cancelled() {
        return None;
    }
    let mut failed = Vec::new();
    for (&i, result) in runnable.iter().zip(&results) {
        task.record_checklist_result(i, result.code == Some(0));
        if !task.checklist[i].passes {
            failed.push(format!(
                "Checklist item \"{}\": `{}` exited {}.\n{}",
                truncate_chars(&task.checklist[i].text, 120),
                result.command,
                result
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                result.tail
            ));
        }
    }
    task.touch(now_ms());
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    (!failed.is_empty()).then(|| failed.join("\n\n"))
}
