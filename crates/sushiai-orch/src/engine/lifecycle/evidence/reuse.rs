use super::StaleEvidence;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::review::evidence::EvidenceScope;
use crate::engine::state::App;
use crate::task::attempts::{self, EvidenceObservation};
use crate::task::{Stage, Task};

pub(super) async fn previous(
    app: &App,
    task: &mut Task,
    run: &RunReceipt,
    scope: &EvidenceScope,
) -> Option<StaleEvidence> {
    let n = task.attempts[run.idx].n;
    let (m, old_tree, old_base, old_evidence) = task
        .attempts
        .iter()
        .filter(|attempt| {
            attempt.stage == Stage::Implement
                && attempt.n < n
                && attempt.evidence_from.is_none()
                && !attempt.evidence.is_empty()
        })
        .filter_map(|attempt| {
            Some((
                attempt.n,
                attempt.evidence_tree.clone()?,
                attempt.evidence_base.clone(),
                attempt.evidence.clone(),
            ))
        })
        .next_back()?;
    let old_evidence: Vec<String> = old_evidence
        .into_iter()
        .filter(|file| scope.allows_saved(file))
        .collect();
    let worktree = run.worktree.clone();
    let base = task.base_sha.clone();
    let diff = tokio::task::spawn_blocking(move || {
        crate::verify::healing::scope::own_changes_since(
            &worktree,
            &old_tree,
            old_base.as_deref(),
            &base,
        )
    })
    .await
    .unwrap_or_else(|error| Err(crate::git::GitError(error.to_string())));
    let Ok(other) = diff else {
        return None;
    };
    if !other.is_empty() || old_evidence.is_empty() {
        return Some(StaleEvidence {
            attempt: m,
            paths: other.into_iter().take(5).collect(),
        });
    }
    attempts::record_evidence(
        task,
        run.idx,
        EvidenceObservation::Reused {
            files: old_evidence,
            from: m,
        },
    );
    task.record_decision(format!(
        "Orchestrator: attempt {n} changed no file of its own since attempt {m} (artifacts, tests and carried-in base commits do not count); reused attempt {m}'s evidence"
    ));
    app.save_task_logged(task);
    app.broadcast_task(task);
    None
}
