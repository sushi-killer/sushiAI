use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::state::App;
use crate::engine::subtask_checks;
use crate::git::paths::short_sha;
use crate::policy::paths::glob_match;
use crate::policy::settings::ScopedCheck;
use crate::policy::settings::Settings;
use crate::task::checks::contribution::ChildChecks;
use crate::task::state::Task;
use crate::verify::scoping::filter_scoped;
use std::path::Path;

/// One merged policy for diff checks, base checks, briefs and shared checks.
pub(in crate::engine) fn scoped_checks_for(settings: &Settings, task: &Task) -> Vec<ScopedCheck> {
    let mut scoped = settings.scoped_checks.clone();
    if let Ok(entries) =
        crate::worktree_env::scoped_checks_at(Path::new(&task.repo), &task.base_sha)
    {
        scoped.extend(entries);
    }
    scoped
}

/// Like [`filter_scoped`] for the task's own diff against `base`. When the
/// diff cannot be read every command runs.
pub(in crate::engine) async fn scope_to_diff(
    app: &App,
    task: &Task,
    worktree: &Path,
    base: &str,
    commands: &[String],
) -> (Vec<String>, Vec<String>) {
    let scoped = scoped_checks_for(&app.settings.read().unwrap(), task);
    if !commands
        .iter()
        .any(|c| scoped.iter().any(|e| glob_match(&e.command, c.trim())))
    {
        return (commands.to_vec(), Vec::new());
    }
    let (wt, b) = (worktree.to_path_buf(), base.to_string());
    let changed =
        tokio::task::spawn_blocking(move || crate::git::changes::changed_files(&wt, &b).ok())
            .await
            .unwrap_or(None);
    match changed {
        Some(changed) => filter_scoped(&scoped, &task.repo, commands, &changed),
        None => (commands.to_vec(), Vec::new()),
    }
}

/// Path policy first, then the planner's check for subtasks gated at their parent.
pub(in crate::engine) async fn scope_task_checks(
    app: &App,
    task: &Task,
    worktree: &Path,
    base: &str,
    commands: &[String],
) -> (Vec<String>, Vec<String>) {
    let (path_commands, mut lines) = scope_to_diff(app, task, worktree, base, commands).await;
    let enabled = task.parent.is_some() && app.settings.read().unwrap().gates_at_parent(&task.repo);
    if !enabled {
        return (path_commands, lines);
    }
    let mut narrowed =
        subtask_checks::scope(worktree, &path_commands, task.brief_check.check.as_deref());
    // Path-scoped shared checks also remain the parent's responsibility.
    narrowed.deferred.extend(
        commands
            .iter()
            .filter(|c| !path_commands.contains(c))
            .cloned(),
    );
    if !narrowed.deferred.is_empty() && !add_parent_checks(app, task, &narrowed.deferred) {
        return (commands.to_vec(), Vec::new());
    }
    lines.extend(narrowed.decisions);
    (narrowed.commands, lines)
}

fn add_parent_checks(app: &App, task: &Task, commands: &[String]) -> bool {
    let Some(parent_id) = task.parent.as_deref() else {
        return false;
    };
    let lock = app.task_lock(parent_id);
    let _guard = lock.lock().unwrap();
    let Ok(Some(mut parent)) = app.store.load_task(parent_id) else {
        return false;
    };
    let signals = app
        .controls
        .lock()
        .unwrap()
        .get(parent_id)
        .filter(|control| !control.signals.cancel.is_cancelled())
        .map(|control| control.signals.clone());
    let contribution = ChildChecks::new(short_sha(&task.id), commands);
    if signals.is_some() || parent.pending_amendment.is_some() {
        let expected = parent.pending_amendment.clone();
        let authority = check_authority(&app.settings.read().unwrap(), &parent);
        if parent
            .queue_final_checks(&authority, contribution, now_ms())
            .is_err()
            || app
                .store
                .save_task_with_pending(&parent, expected.as_ref())
                .is_err()
        {
            return false;
        }
        if let Some(signals) = signals {
            *signals.pending_amend.lock().unwrap() = parent.pending_amendment.clone();
        }
        return true;
    }
    if contribution.append_to(&mut parent, now_ms()) {
        if app.store.save_task(&parent).is_err() {
            return false;
        }
        app.broadcast_task(&parent);
    }
    true
}

/// Splits a subtask's `finalVerify` into the commands it runs and those its
/// parent runs anyway (exact match after trim, against the parent's `verify`
/// and `finalVerify`). Nothing is deferred for a top-level task, a parent that
/// cannot be loaded, or a repo with `gateAt` "every". The stored list is
/// never changed.
pub(in crate::engine) fn split_deferred(
    app: &App,
    settings: &Settings,
    task: &Task,
) -> (Vec<String>, Vec<String>) {
    let all = || (task.final_verify.clone(), Vec::new());
    let Some(parent_id) = task.parent.as_deref() else {
        return all();
    };
    if !settings.gates_at_parent(&task.repo) {
        return all();
    }
    let Ok(Some(parent)) = app.store.load_task(parent_id) else {
        return all();
    };
    let at_parent: Vec<&str> = parent
        .verify
        .iter()
        .chain(&parent.final_verify)
        .map(|c| c.trim())
        .collect();
    let (deferred, run) = task
        .final_verify
        .iter()
        .cloned()
        .partition(|c| at_parent.contains(&c.trim()));
    (run, deferred)
}

/// The decision line for a command deferred to the parent.
pub(in crate::engine) fn deferred_line(command: &str) -> String {
    format!("Orchestrator: Deferred to parent: {command}")
}

#[cfg(test)]
#[path = "scoping/tests.rs"]
mod tests;
