use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;

/// Once a parent holds every subtask, the subtasks' own branches are only
/// clutter: removes, locally, those fully contained in the parent branch.
pub(in crate::engine) async fn remove_chain_branches(app: &Arc<App>, task: &mut Task) {
    let candidates: Vec<(String, String)> = app
        .repo_tasks(&task.repo)
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()) && t.branch != task.branch)
        .map(|t| (t.id.clone(), t.branch.clone()))
        .collect();
    if candidates.is_empty() {
        return;
    }
    let (repo, into) = (PathBuf::from(&task.repo), task.branch.clone());
    let removed = tokio::task::spawn_blocking(move || {
        crate::delivery::git::recovery::delete_merged_branches(&repo, &into, &candidates)
    })
    .await
    .unwrap_or_default();
    if !removed.is_empty() {
        task.record_decision(format!("Branches: removed {}", removed.join(", ")));
    }
}

/// The carry-onto-moved-base failure detail the agent gets: which files
/// conflict and how to resolve them.
pub(in crate::engine) fn conflict_detail(
    base_ref: &str,
    new_sha: &str,
    files: &[String],
    task_id: &str,
) -> String {
    format!(
        "{base_ref} moved ahead to {}, and your changes were carried onto it. \
         These files conflict: {}. Text files carry <<<<<<< / >>>>>>> markers; \
         for a binary or deleted file, your version is `git show {}:<path>`. \
         Resolve each one so both the base's change and yours survive, then finish the task.",
        short_sha(new_sha),
        files.join(", "),
        crate::delivery::git::carry::wip_ref(task_id)
    )
}

/// A relay link's start: its worktree continues the previous link's branch
/// (the earlier subtask's commit), so it edits on top of that work. When the
/// previous link cannot be continued the subtask starts from the parent's
/// head like any other.
pub(in crate::engine) async fn start_relay_link(app: &Arc<App>, task: &mut Task, prev_id: &str) {
    let prev = app
        .store
        .load_task(prev_id)
        .ok()
        .flatten()
        .filter(|p| p.status == TaskStatus::Done);
    let Some(prev) = prev else {
        task.discard_relay();
        task.record_decision(
            "Relay: the previous subtask did not finish; starting from the parent's head instead"
                .to_string(),
        );
        return;
    };
    let (wt, from, tid, branch) = (
        PathBuf::from(&task.worktree),
        task.base_sha.clone(),
        task.id.clone(),
        prev.branch.clone(),
    );
    let synced = tokio::task::spawn_blocking(move || {
        crate::delivery::git::carry::carry_onto_moved_base(&wt, &branch, &from, &tid)
    })
    .await;
    let head = match synced {
        Ok(Ok(crate::delivery::git::carry::Rebase::Moved { new_sha, .. })) => new_sha,
        Ok(Ok(crate::delivery::git::carry::Rebase::Unchanged)) => task.base_sha.clone(),
        _ => {
            task.discard_relay();
            task.record_decision(format!(
                "Relay: could not continue {}; starting from the parent's head instead",
                prev.branch
            ));
            return;
        }
    };
    task.record_decision(format!(
        "Relay: continues \"{}\" on {} at {}",
        prev.title,
        prev.branch,
        short_sha(&head)
    ));
    task.record_relay_base(Some(
        prev.queue
            .relay_base
            .clone()
            .unwrap_or_else(|| prev.base_sha.clone()),
    ));
    crate::task::transitions::move_base(task, head);
    task.record_base_ref(Some(prev.branch.clone()));
}
