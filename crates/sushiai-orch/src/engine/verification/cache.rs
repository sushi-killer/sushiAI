use crate::engine::state::App;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::harness::compiler_cache::CacheRun;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::verify::diff::diff_hash;
use crate::verify::passes::is_interrupted;
use std::path::Path;
use std::sync::Arc;

pub(in crate::engine) struct CachedVerification {
    pub(in crate::engine) plan: CheckSnapshot,
    pub(in crate::engine) tree: String,
    pub(in crate::engine) results: Vec<VerifyOutcome>,
}

/// Cached by a hash of `git diff <base>` + the untracked-file list: if
/// nothing has changed since the last run, reuse completed failures. Passes
/// come from the bounded tree cache; interrupted result sets are not stored.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn run_verify_cached(
    app: &Arc<App>,
    task_id: &str,
    repo: &str,
    worktree: &Path,
    run_dir: &Path,
    base: &str,
    commands: &[String],
    cancel: &CancelToken,
) -> (Vec<VerifyOutcome>, Vec<String>) {
    let snapshot = app
        .store
        .load_task(task_id)
        .ok()
        .flatten()
        .map(|task| crate::task::checks::snapshot::CheckSnapshot::capture(&task));
    let wt = worktree.to_path_buf();
    let b = base.to_string();
    let hash = tokio::task::spawn_blocking(move || diff_hash(&wt, &b))
        .await
        .unwrap_or_default();
    let cached = app
        .verify_cache
        .lock()
        .unwrap()
        .get(task_id)
        .filter(|entry| {
            snapshot.as_ref() == Some(&entry.plan)
                && entry.tree == hash
                && entry.results.iter().map(|r| &r.command).eq(commands.iter())
        })
        .map(|entry| entry.results.clone());
    let sandbox = app.task_sandbox(repo, worktree, run_dir);
    let cache_run = CacheRun::for_commands(worktree, commands).await;
    let runner = CheckRunner::new(app, worktree, sandbox.as_ref(), base, cancel).for_task(task_id);
    let mut results = Vec::new();
    let mut lines = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        let previous = cached.as_ref().and_then(|r| r.get(i));
        let (result, decisions) = runner.run_one(command, command, previous).await;
        results.push(result);
        lines.extend(decisions);
    }
    lines.extend(cache_run.finish("verify").await);
    let task_lock = app.task_lock(task_id);
    let _guard = task_lock.lock().unwrap();
    let current = app.store.load_task(task_id).ok().flatten();
    if snapshot
        .as_ref()
        .zip(current.as_ref())
        .is_some_and(|(snapshot, task)| snapshot.matches(task))
    {
        let mut cache = app.verify_cache.lock().unwrap();
        if !cancel.is_cancelled()
            && results
                .iter()
                .all(|r| r.code.is_some() && !is_interrupted(r))
        {
            cache.insert(
                task_id.to_string(),
                CachedVerification {
                    plan: snapshot.clone().expect("matched stored task"),
                    tree: hash,
                    results: results.clone(),
                },
            );
        } else {
            cache.remove(task_id);
        }
    }
    (results, lines)
}
