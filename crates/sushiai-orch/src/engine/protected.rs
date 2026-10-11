//! The planner's test files and the files the checks name: committed before
//! the first attempt, hashed, denied to the implementer and restored when an
//! attempt changes them anyway.

use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_TEST_FILES: usize = 8;
const MAX_TEST_BYTES: usize = 64 * 1024;

/// The repo-relative form of a planned path, or why it is refused.
fn clean_test_path(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty path");
    }
    if raw.starts_with('/') || raw.starts_with('\\') || raw.as_bytes().get(1) == Some(&b':') {
        return Err("absolute path");
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("path goes up with '..'"),
            ".git" => return Err("path is under .git"),
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Err("empty path");
    }
    Ok(parts.join("/"))
}

/// What of the planner's `tests` is stored: safe repo-relative paths, at
/// most 8 files of at most 64 KiB, each path once. A decision line for every
/// file that is dropped.
pub(super) fn keep_planned_tests(tests: &[PlannedTest]) -> (Vec<PlannedTest>, Vec<String>) {
    let mut kept: Vec<PlannedTest> = Vec::new();
    let mut dropped = Vec::new();
    for t in tests {
        let reason = match clean_test_path(&t.path) {
            Err(why) => Some(why),
            Ok(_) if kept.len() >= MAX_TEST_FILES => Some("more than 8 files"),
            Ok(_) if t.content.len() > MAX_TEST_BYTES => Some("larger than 64 KiB"),
            Ok(path) if kept.iter().any(|k| k.path == path) => Some("listed twice"),
            Ok(path) => {
                kept.push(PlannedTest {
                    path,
                    content: t.content.clone(),
                });
                None
            }
        };
        if let Some(why) = reason {
            dropped.push(format!(
                "Orchestrator: planner test file dropped ({why}): {}",
                t.path.trim()
            ));
        }
    }
    (kept, dropped)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Shell words of `command` that could be a path: split on whitespace and
/// the shell's separators and quotes.
fn command_words(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || ";|&()<>\"'`=,{}".contains(c))
        .filter(|w| !w.is_empty() && !w.starts_with('-'))
        .filter_map(|w| clean_test_path(w).ok())
        .collect()
}

/// Everything the tests commit makes unchangeable: the planner's files and
/// every file in the repository a visible check command names. The
/// held-out command is not read, so its files stay unnamed.
pub(super) fn protected_hashes(
    worktree: &Path,
    rev: &str,
    task: &Task,
) -> BTreeMap<String, String> {
    let mut paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    for check in &task.checks {
        paths.extend(command_words(&check.run));
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|p| {
            git::blob_at(worktree, rev, &p).map(|bytes| {
                let hash = sha256_hex(&bytes);
                (p, hash)
            })
        })
        .collect()
}

/// Carries the task's work and its trusted test commit together, refreshing
/// the test anchor after the carry while keeping implementation uncommitted.
pub(super) fn carry_with_planner_tests(
    task: &mut Task,
    worktree: &Path,
    base_ref: &str,
) -> Result<git::Rebase, git::GitError> {
    let Some(commit) = task
        .planner_tests
        .commit
        .as_deref()
        .filter(|commit| *commit != task.base_sha)
    else {
        return git::carry_onto_moved_base(worktree, base_ref, &task.base_sha, &task.id);
    };
    let paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    let (carried, new_commit) = git::carry_onto_moved_base_with_commit(
        worktree,
        base_ref,
        &task.base_sha,
        &task.id,
        commit,
        &paths,
    )?;
    if matches!(
        carried,
        git::Rebase::Moved { .. } | git::Rebase::Conflicts { .. }
    ) {
        task.planner_tests.protected = Some(protected_hashes(worktree, &new_commit, task));
        task.planner_tests.commit = Some(new_commit);
    }
    Ok(carried)
}

/// The protected files in absolute form, for the deny rules of a run's
/// settings.
pub(super) fn protected_abs_paths(task: &Task, worktree: &Path) -> Vec<String> {
    task.planner_tests
        .protected
        .iter()
        .flat_map(|m| m.keys())
        .map(|p| worktree.join(p).to_string_lossy().to_string())
        .collect()
}

/// Before the first attempt something is left to do: the tests are not
/// committed, a check has no baseline, or the hashes are not stored.
pub(super) fn needs_prepare(task: &Task) -> bool {
    let t = &task.planner_tests;
    (!t.files.is_empty() && t.commit.is_none())
        || needs_baseline(task)
        || (t.protected.is_none() && (!t.files.is_empty() || !task.checks.is_empty()))
}

/// Writes the planner's files into `worktree` and commits just those paths;
/// the new HEAD. Running it again on the result changes nothing.
fn write_and_commit(worktree: &Path, task: &Task) -> Result<String, String> {
    let root = worktree.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for t in &task.planner_tests.files {
        let target = root.join(&t.path);
        let parent = target.parent().unwrap_or(&root);
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let real = parent.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(&root) {
            return Err(format!("{} resolves outside the worktree", t.path));
        }
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{} is a symlink", t.path));
        }
        if std::fs::read(&target).ok().as_deref() != Some(t.content.as_bytes()) {
            std::fs::write(&target, &t.content).map_err(|e| e.to_string())?;
        }
        paths.push(t.path.clone());
    }
    git::commit_paths(
        worktree,
        &paths,
        &format!("test: {} (planner tests)", task.title),
        &task.id,
    )
    .map_err(|e| e.to_string())
}

/// The steps before attempt 1, each idempotent: write and commit the
/// planner's tests, store the commit, baseline the checks on it, store a
/// sha256 per protected file. `false` when cancelled.
pub(super) async fn prepare_tests(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> bool {
    if !task.planner_tests.files.is_empty() && task.planner_tests.commit.is_none() {
        let (wt, snapshot) = (worktree.to_path_buf(), task.clone());
        let done = tokio::task::spawn_blocking(move || write_and_commit(&wt, &snapshot))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        match done {
            Ok(sha) => {
                task.decisions.push(format!(
                    "Orchestrator: committed {} planner test file(s) at {}",
                    task.planner_tests.files.len(),
                    short_sha(&sha)
                ));
                task.planner_tests.commit = Some(sha);
            }
            Err(e) => {
                task.decisions.push(format!(
                    "Orchestrator: planner tests not committed, dropped: {e}"
                ));
                task.planner_tests.files.clear();
            }
        }
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
    }
    if !baseline_checks(app, task_id, task, worktree, cancel).await {
        return false;
    }
    if task.planner_tests.protected.is_none()
        && (!task.planner_tests.files.is_empty() || !task.checks.is_empty())
    {
        let rev = task.work_base().to_string();
        let (wt, snapshot) = (worktree.to_path_buf(), task.clone());
        let hashes = tokio::task::spawn_blocking(move || protected_hashes(&wt, &rev, &snapshot))
            .await
            .unwrap_or_default();
        task.planner_tests.protected = Some(hashes);
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    true
}

/// After an attempt: the protected files that changed since the tests
/// commit or no longer match their stored hash, put back from that commit.
/// `None` when nothing protected was touched.
pub(super) async fn guard_protected(
    task: &Task,
    worktree: &Path,
    changed: &[String],
) -> Option<String> {
    let hashes = task
        .planner_tests
        .protected
        .clone()
        .filter(|m| !m.is_empty())?;
    let (wt, rev, changed) = (
        worktree.to_path_buf(),
        task.work_base().to_string(),
        changed.to_vec(),
    );
    tokio::task::spawn_blocking(move || {
        let mut bad: Vec<String> = hashes
            .iter()
            .filter(|(path, hash)| {
                changed.iter().any(|c| c == *path)
                    || std::fs::read(wt.join(path))
                        .map(|b| sha256_hex(&b) != **hash)
                        .unwrap_or(true)
            })
            .map(|(path, _)| path.clone())
            .collect();
        if bad.is_empty() {
            return None;
        }
        bad.sort();
        let _ = git::restore_paths(&wt, &rev, &bad);
        Some(format!(
            "The attempt changed protected files: {}. They are the task's tests and the files its checks name; orchd put them back. Make the change in other files and leave these as they are.",
            bad.join(", ")
        ))
    })
    .await
    .unwrap_or(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(path: &str, content: &str) -> PlannedTest {
        PlannedTest {
            path: path.into(),
            content: content.into(),
        }
    }

    #[test]
    fn only_repo_relative_paths_inside_the_limits_are_kept() {
        let big = "x".repeat(MAX_TEST_BYTES + 1);
        let mut input = vec![
            test("/etc/passwd", "a"),
            test("../up.rs", "a"),
            test("a/../b.rs", "a"),
            test(".git/hooks/pre-commit", "a"),
            test("sub/.git/config", "a"),
            test("  ", "a"),
            test("big.rs", &big),
            test("./tests/a.rs", "a"),
            test("tests/a.rs", "again"),
        ];
        let (kept, dropped) = keep_planned_tests(&input);
        assert_eq!(kept, vec![test("tests/a.rs", "a")]);
        assert_eq!(dropped.len(), 8, "{dropped:?}");
        assert!(dropped.iter().any(|d| d.contains("larger than 64 KiB")));
        input = (0..10).map(|i| test(&format!("t{i}.rs"), "")).collect();
        let (kept, dropped) = keep_planned_tests(&input);
        assert_eq!(kept.len(), 8);
        assert_eq!(dropped.len(), 2);
    }

    #[test]
    fn command_words_skip_flags_and_split_on_shell_punctuation() {
        assert_eq!(
            command_words("node --test tests/a.cjs && grep -q 'x' ./src/b.ts; cat <in/c.txt"),
            vec![
                "node",
                "tests/a.cjs",
                "grep",
                "x",
                "src/b.ts",
                "cat",
                "in/c.txt"
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_landing_check_keeps_the_carried_test_anchor_for_retry() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = git::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/landing", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha;
        task.base_ref = Some(base_ref.clone());
        task.planner_tests.files = vec![test("tests/planner.sh", "true\n")];
        let id = task.id.clone();
        let cancel = CancelToken::new();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &cancel).await);
        let old_tests = task.work_base().to_string();
        let old_hashes = task.planner_tests.protected.clone();
        std::fs::write(wt.join("implementation.txt"), "implementation\n").unwrap();
        std::fs::write(repo.join("FIXED.txt"), "fixed\n").unwrap();
        let new_base =
            git::commit_paths(&repo, &["FIXED.txt".to_string()], "fix: base", "base").unwrap();
        task.verify = vec!["false".to_string()];
        let run_dir = dir.path().join("runs");
        let result = carry_and_check(
            &app,
            &mut task,
            None,
            &wt,
            &run_dir,
            &base_ref,
            "Another task",
            &[],
            &cancel,
        )
        .await;
        assert!(matches!(
            result,
            Err(Landing::Failed {
                kind: FailureKind::Verify,
                ..
            })
        ));
        assert_eq!(task.base_sha, new_base);
        assert_ne!(task.work_base(), old_tests);
        assert_eq!(git::head_sha(&wt).unwrap(), task.work_base());
        assert_eq!(
            git_in(&wt, &["rev-parse", &format!("{}^", task.work_base())]),
            new_base
        );
        assert_eq!(task.planner_tests.protected, old_hashes);
        let changed = git::changed_files(&wt, task.work_base()).unwrap();
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert!(wt.join("FIXED.txt").exists() && wt.join("implementation.txt").exists());

        task.verify.clear();
        assert!(matches!(
            carry_and_check(
                &app,
                &mut task,
                None,
                &wt,
                &run_dir,
                &base_ref,
                "Another task",
                &[],
                &cancel,
            )
            .await,
            Ok(false)
        ));
        assert_eq!(git::head_sha(&wt).unwrap(), new_base);
        let pending = git::changed_files(&wt, &new_base).unwrap();
        assert!(pending.contains(&"tests/planner.sh".to_string()));
        assert!(pending.contains(&"implementation.txt".to_string()));
    }
}
