use crate::git::paths::short_sha;
use crate::task::PlannerTests;
use crate::task::Task;
use std::path::Path;

use super::prepare::write_files;
use super::restore::{refresh_protected, undo_tests_commit};
/// Carries the agent's work onto the moved base. The planner's files are not
/// part of that work: they are taken out of the tree first, so a path the base
/// added since the task began cannot conflict, and written again after the
/// carry whatever it did.
pub(in crate::engine) fn carry_with_planner_tests(
    task: &mut Task,
    worktree: &Path,
    base_ref: &str,
) -> Result<crate::delivery::git::carry::Rebase, crate::git::GitError> {
    let _ = undo_tests_commit(task, worktree);
    let paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    crate::git::changes::drop_paths(worktree, &task.base_sha, &paths)?;
    let carried = crate::delivery::git::carry::carry_onto_moved_base(
        worktree,
        base_ref,
        &task.base_sha,
        &task.id,
    );
    let written = write_files(worktree, &task.planner_tests.files).map_err(crate::git::GitError);
    let carried = carried?;
    written?;
    refresh_protected(task, worktree, &carried);
    Ok(carried)
}

/// Stores the planner-tests state a carry returned, with one decision line for
/// every protected file whose hash the new base changed.
pub(in crate::engine) fn adopt_carried_tests(
    task: &mut Task,
    tests: PlannerTests,
    carried: &crate::delivery::git::carry::Rebase,
) {
    if let crate::delivery::git::carry::Rebase::Moved { new_sha, .. }
    | crate::delivery::git::carry::Rebase::Conflicts { new_sha, .. } = carried
    {
        let old = task.planner_tests.protected.clone().unwrap_or_default();
        for (path, hash) in tests.protected.iter().flatten() {
            if old.get(path).is_some_and(|h| h != hash) {
                task.record_decision(format!(
                    "Orchestrator: protected {path} now matches the new base {}",
                    short_sha(new_sha)
                ));
            }
        }
    }
    crate::task::checks::edits::adopt_planner_tests(task, tests);
}
