use crate::engine::lifecycle::changes::record_changed_files;
use crate::engine::lifecycle::native_result::ObservedRun;
use crate::engine::protected::restore::guard_protected;
use crate::task::{attempts, Task};

pub(super) async fn protect(task: &mut Task, observed: &ObservedRun) -> Vec<String> {
    let mut changed = record_changed_files(task, &observed.run).await;
    let restored = guard_protected(task, &observed.run.worktree, &changed)
        .await
        .unwrap_or_default();
    if !restored.is_empty() {
        task.record_decision(format!(
            "Orchestrator: discarded the attempt's edits to protected files: {}; checks run on the originals",
            restored.join(", ")
        ));
        changed.retain(|file| !restored.contains(file));
        attempts::record_changed_files(task, observed.run.idx, changed.clone());
    }
    let mut discarded = restored;
    discarded.extend(observed.discarded.iter().cloned());
    discarded.sort();
    discarded.dedup();
    task.record_discarded_tests(discarded);
    changed
}
