mod reuse;
pub(super) mod validation;

use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::review::evidence::{evidence_scope, save_evidence};
use crate::engine::state::App;
use crate::task::attempts::{self, EvidenceObservation};
use crate::task::Task;

pub(super) struct StaleEvidence {
    attempt: u32,
    paths: Vec<String>,
}

pub(super) async fn capture(app: &App, task: &mut Task, run: &RunReceipt) -> Option<StaleEvidence> {
    let since = task.attempts[run.idx].started_at;
    let scope = evidence_scope(task);
    let saved = save_evidence(&run.worktree, since, &run.run_dir.join("evidence"), &scope);
    if saved.is_empty() {
        if !task.visual_criteria_texts().is_empty() {
            return reuse::previous(app, task, run, &scope).await;
        }
        return None;
    }
    let worktree = run.worktree.clone();
    let tree =
        tokio::task::spawn_blocking(move || crate::git::changes::worktree_tree(&worktree).ok())
            .await
            .unwrap_or(None);
    let base = task.base_sha.clone();
    attempts::record_evidence(
        task,
        run.idx,
        EvidenceObservation::Captured {
            files: saved,
            tree,
            base,
        },
    );
    app.save_task_logged(task);
    app.broadcast_task(task);
    None
}
