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

/// What [`guard_protected`] did: the protected files it put back, and the
/// protected files it kept because they only grew.
#[derive(Debug, Default)]
pub(in crate::engine) struct Guarded {
    pub(in crate::engine) restored: Vec<String>,
    pub(in crate::engine) grown: Vec<String>,
}

/// The lines of `bytes`, without the empty piece after a final newline.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    body.split(|b| *b == b'\n').collect()
}

/// Whether `new` keeps every line of `old`, unchanged and in order, and adds
/// at least one byte.
fn only_grew(old: &[u8], new: &[u8]) -> bool {
    if old == new {
        return false;
    }
    let mut kept = lines(new).into_iter();
    lines(old).into_iter().all(|line| kept.any(|n| n == line))
}

/// Whether the protected `path`, not one of the planner's files, only grew:
/// it exists in the base with the stored hash and the worktree copy keeps
/// every base line in order.
fn grew_from_base(worktree: &Path, base: &str, path: &str, hash: &str) -> bool {
    let Some(old) = crate::git::history::blob_at(worktree, base, path) else {
        return false;
    };
    sha256_hex(&old) == hash
        && std::fs::read(worktree.join(path)).is_ok_and(|new| only_grew(&old, &new))
}

/// The protected files the worktree holds grown: every file of the store that
/// is not one of the planner's files and only grew from the base. The review
/// is shown their added lines.
pub(in crate::engine) fn grown_paths(task: &Task, worktree: &Path) -> Vec<String> {
    let Some(hashes) = task.planner_tests.protected.as_ref() else {
        return Vec::new();
    };
    hashes
        .iter()
        .filter(|(path, _)| !task.planner_tests.files.iter().any(|t| t.path == **path))
        .filter(|(path, hash)| grew_from_base(worktree, &task.base_sha, path, hash))
        .map(|(path, _)| path.clone())
        .collect()
}

/// The decision line for protected files kept because they only grew.
pub(in crate::engine) fn grown_line(grown: &[String]) -> Option<String> {
    (!grown.is_empty()).then(|| {
        format!(
            "Orchestrator: kept the attempt's additions to protected files: {}; every existing line is unchanged and the review checks that the additions do not weaken a test",
            grown.join(", ")
        )
    })
}

/// After an attempt (and after a carry, before the checks and the landing
/// commit): the protected files that changed or no longer match their stored
/// hash are put back, unless the change only added lines. The planner's
/// files are strict: any change is undone and they are written from the
/// store. Any other protected file that exists in the base may grow: when
/// every base line is still there, in order, the file is kept and reported
/// as grown; a changed or removed line restores it from the base.
pub(in crate::engine) async fn guard_protected(
    task: &Task,
    worktree: &Path,
    changed: &[String],
) -> Guarded {
    let Some(hashes) = task
        .planner_tests
        .protected
        .clone()
        .filter(|m| !m.is_empty())
    else {
        return Guarded::default();
    };
    let (wt, base, changed, planned) = (
        worktree.to_path_buf(),
        task.base_sha.clone(),
        changed.to_vec(),
        task.planner_tests.files.clone(),
    );
    tokio::task::spawn_blocking(move || {
        let touched: Vec<(String, String)> = hashes
            .into_iter()
            .filter(|(path, hash)| {
                let current = std::fs::read(wt.join(path)).ok();
                changed.iter().any(|c| c == path)
                    || current.as_ref().is_none_or(|b| sha256_hex(b) != *hash)
            })
            .collect();
        let mut guarded = Guarded::default();
        for (path, hash) in touched {
            let mine = planned.iter().any(|t| t.path == path);
            if !mine && grew_from_base(&wt, &base, &path, &hash) {
                guarded.grown.push(path);
            } else {
                guarded.restored.push(path);
            }
        }
        let (mine, others): (Vec<String>, Vec<String>) = guarded
            .restored
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
        guarded
    })
    .await
    .unwrap_or_default()
}
