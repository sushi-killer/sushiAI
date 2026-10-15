use crate::task::PlannedTest;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;

use super::paths::protected_paths;
use super::prepare::write_files;
use super::snapshot::{protected_hashes, sha256_hex};
/// A task an older build prepared holds its planner tests in a commit on its
/// branch. The commit is undone once: the files stay in the tree, the stored
/// content is written over them and the record is cleared. A branch that is
/// final (done, landed, with a pull request) is left as it is. The decision
/// line when the record was cleared.
pub(super) fn undo_tests_commit(
    task: &mut Task,
    worktree: &Path,
) -> Result<Option<String>, String> {
    if task.planner_tests.commit.is_none()
        || task.status == TaskStatus::Done
        || task.pr_url.is_some()
        || task.landed_sha.is_some()
    {
        return Ok(None);
    }
    if crate::git::history::is_ancestor(worktree, &task.base_sha, "HEAD") == Some(true) {
        crate::delivery::git::carry::uncommit_to(worktree, &task.base_sha)
            .map_err(|e| e.to_string())?;
    }
    let stored = task.planner_tests.protected.clone().unwrap_or_default();
    let differing: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .filter(|f| {
            let current = std::fs::read(worktree.join(&f.path)).ok();
            stored
                .get(&f.path)
                .is_some_and(|h| current.is_none_or(|b| sha256_hex(&b) != *h))
        })
        .map(|f| f.path.clone())
        .collect();
    write_files(worktree, &task.planner_tests.files)?;
    task.forget_planner_commit();
    Ok(Some(if differing.is_empty() {
        "Orchestrator: the planner tests are no longer a commit of the branch; they are stored with the task".to_string()
    } else {
        format!(
            "Orchestrator: the planner tests are no longer a commit of the branch; {} did not match its stored hash and was written again",
            differing.join(", ")
        )
    }))
}

/// Before every attempt: a tests commit of an older build is undone
/// ([`undo_tests_commit`]), and files that are no longer protected are
/// dropped from the stored set.
pub(in crate::engine) async fn repair_protection(task: &mut Task, worktree: &Path) {
    let (wt, mut snapshot) = (worktree.to_path_buf(), task.clone());
    let undone = tokio::task::spawn_blocking(move || {
        let line = undo_tests_commit(&mut snapshot, &wt);
        (line, snapshot.planner_tests.clone())
    })
    .await;
    match undone {
        Ok((Ok(Some(line)), tests)) => {
            crate::task::checks::edits::adopt_planner_tests(task, tests);
            task.record_decision(line);
        }
        Ok((Err(e), _)) => {
            let line = format!("Orchestrator: the planner tests commit was not undone: {e}");
            if !task.decisions.contains(&line) {
                task.record_decision(line);
            }
        }
        _ => {}
    }
    if let Some(stored) = task.planner_tests.protected.clone() {
        let keep = protected_paths(task);
        let dropped: Vec<String> = stored
            .keys()
            .filter(|p| !keep.contains(p))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            let pruned = stored
                .into_iter()
                .filter(|(p, _)| keep.contains(p))
                .collect();
            task.record_protected_tests(pruned);
            task.record_decision(format!(
                "Orchestrator: no longer protecting {}: the task is asked to change it",
                dropped.join(", ")
            ));
        }
    }
}

/// After a carry: the hashes of the files the checks name are those of the new
/// head, so a file the new base changed is not mistaken for the agent's edit
/// and put back to the old content.
pub(super) fn refresh_protected(
    task: &mut Task,
    worktree: &Path,
    carried: &crate::delivery::git::carry::Rebase,
) {
    if let crate::delivery::git::carry::Rebase::Moved { new_sha, .. }
    | crate::delivery::git::carry::Rebase::Conflicts { new_sha, .. } = carried
    {
        if task.planner_tests.protected.is_some() {
            task.record_protected_tests(protected_hashes(worktree, new_sha, task));
        }
    }
}

/// After an attempt (and after a carry, before the checks and the landing
/// commit): the protected files that changed or no longer match their stored
/// hash are put back. The planner's files are written from the store, the
/// others restored from the base. Returns the files it restored; `None` when
/// nothing protected was touched.
pub(in crate::engine) async fn guard_protected(
    task: &Task,
    worktree: &Path,
    changed: &[String],
) -> Option<Vec<String>> {
    let hashes = task
        .planner_tests
        .protected
        .clone()
        .filter(|m| !m.is_empty())?;
    let (wt, base, changed, planned) = (
        worktree.to_path_buf(),
        task.base_sha.clone(),
        changed.to_vec(),
        task.planner_tests.files.clone(),
    );
    tokio::task::spawn_blocking(move || {
        let mut bad: Vec<String> = hashes
            .iter()
            .filter(|(path, hash)| {
                let current = std::fs::read(wt.join(path)).ok();
                changed.iter().any(|c| c == *path)
                    || current.as_ref().is_none_or(|b| sha256_hex(b) != **hash)
            })
            .map(|(path, _)| path.clone())
            .collect();
        if bad.is_empty() {
            return None;
        }
        bad.sort();
        let (mine, others): (Vec<String>, Vec<String>) = bad
            .iter()
            .cloned()
            .partition(|p| planned.iter().any(|t| t.path == *p));
        if !others.is_empty() {
            let _ = crate::git::changes::restore_paths(&wt, &base, &others);
        }
        let files: Vec<PlannedTest> = planned
            .into_iter()
            .filter(|t| mine.contains(&t.path))
            .collect();
        let _ = write_files(&wt, &files);
        Some(bad)
    })
    .await
    .unwrap_or(None)
}
