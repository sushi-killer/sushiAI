use crate::engine::delivery::pr::publish::open_pr_on_done;
use crate::engine::state::App;
use crate::task::state::Task;
use crate::task::state::TaskStatus;
use std::sync::Arc;
use std::time::Duration;

/// Tells the owner in the repo's chat, off the caller's path: the chat can be
/// busy with a live turn, so a refused post is tried again a few times.
pub(in crate::engine) fn notify_owner(app: &Arc<App>, repo: &str, text: String) {
    let (app, repo) = (app.clone(), repo.to_string());
    tokio::spawn(async move {
        for _ in 0..20 {
            if crate::engine::chat::store::post_note(&app, &repo, &text) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

/// Marks a task that is about to be saved `done` as owing a pull request:
/// a top-level task whose work did not land on its base, unless the setting
/// is off for its repo. Saved with `done`, so a restart can resume the flow.
pub(in crate::engine) fn mark_pr_pending(app: &App, task: &mut Task) {
    let pending = task.parent.is_none()
        && task.pr_url.is_none()
        && task.landed_sha.is_none()
        && !task.archived
        && (task.queue.pr_conflict_tried
            || app.settings.read().unwrap().may_open_pr_on_done(&task.repo));
    task.record_pr_pending(pending);
}

/// Starts the draft and the pull request for a done task that owes one
/// (`pr_pending`): a landing refused on the default branch, or a restart that
/// interrupted the flow.
pub(in crate::engine) fn spawn_pr_on_done(app: &App, task: &Task) {
    if !task.pr_pending || task.status != TaskStatus::Done || task.parent.is_some() {
        return;
    }
    let id = task.id.clone();
    if task.pr_url.is_some()
        || task.archived
        || !(task.queue.pr_conflict_tried
            || app.settings.read().unwrap().may_open_pr_on_done(&task.repo))
    {
        let _ = app.update_task(&id, |t| t.finish_pr());
        return;
    }
    spawn_open_pr(app, &id);
}

/// Runs the flow in the background, without its caller waiting.
pub(in crate::engine) fn spawn_open_pr(app: &App, id: &str) {
    let (app, id) = (app.arc(), id.to_string());
    tokio::spawn(async move { open_pr_on_done(&app, &id).await });
}

/// Adds decision lines and spend to the task and settles the pending mark
/// when the flow `ends`.
pub(super) fn record(app: &App, id: &str, lines: Vec<String>, cost: f64, ends: bool) {
    let _ = app.update_task(id, |task| {
        task.record_decisions(lines);
        task.charge(cost);
        // A task sent back for its resolution keeps its flag and plan.
        if ends && task.status == TaskStatus::Done {
            task.finish_pr();

            task.clear_pr_conflict();
        }
    });
}

/// The task, while it still owes the automatic pull request: done, pending,
/// not landed, none open, not archived. Landing, a pull request opened by
/// someone else or an archive ends the flow without a word.
pub(super) fn owed(app: &App, id: &str) -> Option<Task> {
    let task = app.store.load_task(id).ok().flatten()?;
    (task.status == TaskStatus::Done
        && task.pr_pending
        && task.landed_sha.is_none()
        && task.pr_url.is_none()
        && !task.archived)
        .then_some(task)
}
