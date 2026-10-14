//! The planner's test files and the files the checks name: committed before
//! the first attempt, hashed, denied to the implementer and restored when an
//! attempt changes them anyway.

use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_TEST_FILES: usize = 8;
pub(super) const MAX_TEST_BYTES: usize = 64 * 1024;

/// The repo-relative form of a planned path, or why it is refused.
pub(super) fn clean_test_path(raw: &str) -> Result<String, &'static str> {
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

/// Whether `path` is a document: a check reads it, the task writes it.
fn is_document(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    DOC_EXTENSIONS.contains(&ext.as_str())
}

const DOC_EXTENSIONS: [&str; 3] = ["md", "mdx", "rst"];

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
    // A document a check names is not protected: the task is asked to write it.
    let planned = paths.clone();
    for check in &task.checks {
        paths.extend(
            command_words(&check.run)
                .into_iter()
                .filter(|p| planned.contains(p) || !is_document(p)),
        );
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
        let carried = git::carry_onto_moved_base(worktree, base_ref, &task.base_sha, &task.id)?;
        refresh_protected(task, worktree, &carried);
        return Ok(carried);
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

/// After a carry without planner tests: the hashes of the files the checks
/// name are those of the new head, so a file the new base changed is not
/// mistaken for the agent's edit and put back to the old content.
fn refresh_protected(task: &mut Task, worktree: &Path, carried: &git::Rebase) {
    if let git::Rebase::Moved { new_sha, .. } | git::Rebase::Conflicts { new_sha, .. } = carried {
        if task.planner_tests.protected.is_some() {
            task.planner_tests.protected = Some(protected_hashes(worktree, new_sha, task));
        }
    }
}

/// Stores the planner-tests state a carry returned, with one decision line for
/// every protected file whose hash the new base changed.
pub(super) fn adopt_carried_tests(task: &mut Task, tests: PlannerTests, carried: &git::Rebase) {
    if let git::Rebase::Moved { new_sha, .. } | git::Rebase::Conflicts { new_sha, .. } = carried {
        let old = task.planner_tests.protected.clone().unwrap_or_default();
        for (path, hash) in tests.protected.iter().flatten() {
            if old.get(path).is_some_and(|h| h != hash) {
                task.decisions.push(format!(
                    "Orchestrator: protected {path} now matches the new base {}",
                    short_sha(new_sha)
                ));
            }
        }
    }
    task.planner_tests = tests;
}

/// Before the first attempt something is left to do: the tests are not
/// committed, a check has no baseline, or the hashes are not stored.
pub(super) fn needs_prepare(task: &Task) -> bool {
    let t = &task.planner_tests;
    (!t.files.is_empty() && t.commit.is_none())
        || needs_baseline(task)
        || (t.protected.is_none() && (!t.files.is_empty() || !task.checks.is_empty()))
}

/// The planner-tests commit title: `test: planner tests for <subject>`, the
/// task title without its own conventional-commit type.
pub(super) fn planner_tests_title(title: &str) -> String {
    let subject = title
        .split_once(": ")
        .filter(|(kind, _)| {
            let kind = kind.trim_end_matches('!');
            let kind = kind.split_once('(').map_or(kind, |(k, _)| k);
            !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase())
        })
        .map_or(title, |(_, rest)| rest);
    format!("test: planner tests for {}", subject.trim())
}

/// Writes the planner's files into `worktree`; their repo-relative paths.
fn write_files(worktree: &Path, task: &Task) -> Result<Vec<String>, String> {
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
    Ok(paths)
}

/// How long the repo's format command may run over the planner's files.
const FORMAT_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs the repo's format command (read at `rev`) over `paths` in `worktree`.
/// `Ok(false)`: the repo declares none.
async fn run_repo_format(
    app: &App,
    task: &Task,
    rev: &str,
    worktree: &Path,
    paths: &[String],
    cancel: &CancelToken,
) -> Result<bool, String> {
    let Some(command) = crate::worktree_env::format_command_at(Path::new(&task.repo), rev)? else {
        return Ok(false);
    };
    let run_dir = app.store.task_dir(&task.id).join("runs/format");
    let _ = std::fs::create_dir_all(&run_dir);
    let sandbox = app.task_sandbox(&task.repo, worktree, &run_dir);
    crate::worktree_env::run_format(
        &command,
        paths,
        Path::new(&task.repo),
        worktree,
        &augmented_path(),
        FORMAT_TIMEOUT,
        |script| verify::build_verify_command(worktree, script, sandbox.as_ref(), ""),
        cancel.cancelled(),
    )
    .await
    .map(|()| true)
}

/// Runs the repo's declared format command (read at `rev`) over the written
/// planner files and stores what it left in `task.planner_tests.files`, so the
/// commit, the hashes and the task agree. One decision line, or none without a
/// command.
async fn format_planner_tests(
    app: &App,
    task: &mut Task,
    worktree: &Path,
    paths: &[String],
    rev: &str,
    cancel: &CancelToken,
) -> Option<String> {
    if let Err(e) = run_repo_format(app, task, rev, worktree, paths, cancel).await {
        for t in &task.planner_tests.files {
            let _ = std::fs::write(worktree.join(&t.path), &t.content);
        }
        return Some(format_failed(&e));
    }
    let mut changed = 0;
    for t in &mut task.planner_tests.files {
        if let Ok(text) = std::fs::read_to_string(worktree.join(&t.path)) {
            if text != t.content && text.len() <= MAX_TEST_BYTES {
                t.content = text;
                changed += 1;
            }
        }
    }
    (changed > 0).then(|| {
        format!(
            "Orchestrator: formatted {changed} planner test file(s) with the repo's format command"
        )
    })
}

/// Work a crash left set aside while the planner tests were formatted comes
/// back before anything else touches the tree.
pub(super) async fn recover_format_aside(task: &mut Task, worktree: &Path) {
    let (wt, id) = (worktree.to_path_buf(), task.id.clone());
    match tokio::task::spawn_blocking(move || git::recover_format_aside(&wt, &id))
        .await
        .unwrap_or_else(|e| Err(git::GitError(e.to_string())))
    {
        Ok(true) => task.decisions.push(
            "Orchestrator: put back the work that an interrupted format run had set aside"
                .to_string(),
        ),
        Ok(false) => {}
        Err(e) => task.decisions.push(format!(
            "Orchestrator: could not put back the work set aside for formatting ({e})"
        )),
    }
}

/// After a carry onto a new base the planner commit is a new commit, and the
/// base may have a new formatter or toolchain. The agent's uncommitted work is
/// set aside first, so the format command sees only the new base and the
/// planner commit; it then runs over the planner files and the result is
/// folded into that commit, the stored commit and hashes are refreshed and
/// the work comes back. Nothing changes when the repo has no format command,
/// the files are already clean or the run is cancelled.
pub(super) async fn reformat_carried_tests(
    app: &App,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) {
    let Some(commit) = task.planner_tests.commit.clone() else {
        return;
    };
    if task.planner_tests.files.is_empty() || cancel.is_cancelled() {
        return;
    }
    if !matches!(
        crate::worktree_env::format_command_at(Path::new(&task.repo), &commit),
        Ok(Some(_))
    ) {
        return;
    }
    let (wt, id, head) = (worktree.to_path_buf(), task.id.clone(), commit.clone());
    let aside = tokio::task::spawn_blocking(move || {
        if git::head_sha(&wt)? != head {
            return Ok(None);
        }
        git::set_work_aside(&wt, &id).map(Some)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    let aside = match aside {
        Ok(Some(aside)) => aside,
        Ok(None) => return,
        Err(e) => {
            task.decisions.push(format!(
                "Orchestrator: could not set the work aside to format the planner tests ({e})"
            ));
            return;
        }
    };
    let paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    let line = format_planner_tests(app, task, worktree, &paths, &commit, cancel).await;
    let (wt, p, skip) = (worktree.to_path_buf(), paths, cancel.is_cancelled());
    let amended = tokio::task::spawn_blocking(move || {
        if skip {
            git::discard_paths(&wt, &p)?;
            Ok(None)
        } else {
            git::amend_paths(&wt, &p)
        }
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    let (wt, id) = (worktree.to_path_buf(), task.id.clone());
    let back = match aside {
        Some(aside) => tokio::task::spawn_blocking(move || git::bring_work_back(&wt, &id, &aside))
            .await
            .unwrap_or_else(|e| Err(git::GitError(e.to_string()))),
        None => Ok(()),
    };
    if let Err(e) = back {
        task.decisions.push(format!(
            "Orchestrator: could not put the work back after formatting the planner tests ({e}); it is kept at {}",
            git::format_aside_ref(&task.id)
        ));
    }
    match amended {
        Ok(Some(new_commit)) => {
            let (wt, snapshot, rev) = (worktree.to_path_buf(), task.clone(), new_commit.clone());
            task.planner_tests.protected =
                tokio::task::spawn_blocking(move || protected_hashes(&wt, &rev, &snapshot))
                    .await
                    .ok();
            task.planner_tests.commit = Some(new_commit.clone());
            task.decisions.push(format!(
                "Orchestrator: formatted the planner tests again after the carry, now at {}",
                short_sha(&new_commit)
            ));
        }
        Ok(None) => {}
        Err(e) => task.decisions.push(format!(
            "Orchestrator: could not fold the formatted planner tests into their commit ({e})"
        )),
    }
    if let Some(line) = line.filter(|l| l.contains("failed") && !cancel.is_cancelled()) {
        task.decisions.push(line);
    }
}

fn format_failed(reason: &str) -> String {
    let first = reason.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let short: String = first.chars().take(120).collect();
    format!("Orchestrator: format command failed on planner tests ({short}), committed as written")
}

/// Writes the planner's files, formats them when the repo declares a format
/// command and commits just those paths; the new HEAD and the decision lines.
/// Running it again on the result changes nothing.
async fn write_format_commit(
    app: &App,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> Result<(String, Vec<String>), String> {
    let (wt, snapshot) = (worktree.to_path_buf(), task.clone());
    let paths = tokio::task::spawn_blocking(move || write_files(&wt, &snapshot))
        .await
        .unwrap_or_else(|e| Err(e.to_string()))?;
    let rev = task.base_sha.clone();
    let mut lines: Vec<String> = format_planner_tests(app, task, worktree, &paths, &rev, cancel)
        .await
        .into_iter()
        .collect();
    let (wt, title, id) = (worktree.to_path_buf(), task.title.clone(), task.id.clone());
    let sha = tokio::task::spawn_blocking(move || {
        git::commit_paths(&wt, &paths, &planner_tests_title(&title), &id).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))?;
    lines.push(format!(
        "Orchestrator: committed {} planner test file(s) at {}",
        task.planner_tests.files.len(),
        short_sha(&sha)
    ));
    Ok((sha, lines))
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
        match write_format_commit(app, task, worktree, cancel).await {
            Ok((sha, lines)) => {
                task.decisions.extend(lines);
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

/// Files where whitespace carries meaning: a reformat there is a change.
const WHITESPACE_SIGNIFICANT: [&str; 7] = ["py", "yaml", "yml", "mk", "haml", "pug", "coffee"];

/// Whether `current` is the protected version of `path` at `rev` with only
/// whitespace outside string literals changed: a formatter's work (a newer
/// rustfmt or prettier), not a change to what the test checks.
fn reformatted_only(worktree: &Path, rev: &str, path: &str, current: Option<&[u8]>) -> bool {
    let name = Path::new(path);
    let ext = name.extension().and_then(|e| e.to_str()).unwrap_or("");
    let file = name.file_name().and_then(|f| f.to_str()).unwrap_or("");
    if WHITESPACE_SIGNIFICANT.contains(&ext) || file == "Makefile" {
        return false;
    }
    let (Some(current), Some(original)) = (current, git::blob_at(worktree, rev, path)) else {
        return false;
    };
    match (std::str::from_utf8(current), std::str::from_utf8(&original)) {
        (Ok(a), Ok(b)) => a != b && without_layout(a) == without_layout(b),
        _ => false,
    }
}

/// `text` without whitespace outside quotes (`"`, `'`, `` ` ``); inside them
/// every character counts, so a reformat may not change what a test asserts.
/// A Rust lifetime opens a quote too, which only makes the check stricter.
fn without_layout(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut quote, mut escaped) = (None, false);
    for c in text.chars() {
        if let Some(open) = quote {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
        } else if matches!(c, '"' | '\'' | '`') {
            quote = Some(c);
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

/// After an attempt: the protected files that changed since the tests
/// commit or no longer match their stored hash, put back from that commit.
/// Returns the files it restored; `None` when nothing protected was touched.
pub(super) async fn guard_protected(
    task: &Task,
    worktree: &Path,
    changed: &[String],
) -> Option<Vec<String>> {
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
                let current = std::fs::read(wt.join(path)).ok();
                let touched = changed.iter().any(|c| c == *path)
                    || current.as_ref().is_none_or(|b| sha256_hex(b) != **hash);
                touched && !reformatted_only(&wt, &rev, path, current.as_deref())
            })
            .map(|(path, _)| path.clone())
            .collect();
        if bad.is_empty() {
            return None;
        }
        bad.sort();
        let _ = git::restore_paths(&wt, &rev, &bad);
        Some(bad)
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

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        for (path, text) in files {
            let target = repo.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text).unwrap();
        }
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "files"]);
        (dir, repo)
    }

    #[test]
    fn a_document_a_check_names_stays_writable_and_a_test_file_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("docs/guide.md", "old\n"),
            ("checks/t.test", "x\n"),
            ("notes/readme.rst", "r\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec!["Guide documents it -- check: grep -q parser docs/guide.md".into()];
        task.checks = vec![Check {
            criterion: 0,
            run: "grep -q parser docs/guide.md notes/readme.rst && run-tests checks/t.test".into(),
            baseline: None,
        }];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["checks/t.test"]);
    }

    #[tokio::test]
    async fn the_guard_restores_an_edited_test_and_reports_which_files() {
        let (_dir, repo) = repo_with(&[("t/a.test", "one\n"), ("t/b.test", "two\n")]);
        let mut task = task_with_status(TaskStatus::Running);
        task.planner_tests.files = vec![test("t/a.test", "one\n"), test("t/b.test", "two\n")];
        task.planner_tests.protected = Some(protected_hashes(&repo, "HEAD", &task));
        task.planner_tests.commit = Some(git::head_sha(&repo).unwrap());
        std::fs::write(repo.join("t/a.test"), "edited\n").unwrap();
        std::fs::remove_file(repo.join("t/b.test")).unwrap();
        let restored = guard_protected(&task, &repo, &[]).await.unwrap();
        assert_eq!(restored, vec!["t/a.test", "t/b.test"]);
        assert_eq!(
            std::fs::read_to_string(repo.join("t/a.test")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("t/b.test")).unwrap(),
            "two\n"
        );
        assert!(guard_protected(&task, &repo, &[]).await.is_none());
    }

    #[tokio::test]
    async fn a_carry_refreshes_the_protected_hashes_of_check_files_from_the_new_head() {
        let (_app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        std::fs::write(repo.join("check.sh"), "old\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "check"]);
        let base_ref = git::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/carry", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha;
        task.base_ref = Some(base_ref.clone());
        task.criteria = vec!["it works".into()];
        task.checks = vec![Check {
            criterion: 0,
            run: "sh check.sh".into(),
            baseline: None,
        }];
        task.planner_tests.protected = Some(protected_hashes(&wt, &task.base_sha, &task));
        // The base moves and changes the protected file; the task did not.
        std::fs::write(repo.join("check.sh"), "new\n").unwrap();
        git_in(&repo, &["commit", "-qam", "newer check"]);
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();
        let mut snapshot = task.clone();
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &base_ref).unwrap();
        assert!(matches!(carried, git::Rebase::Moved { .. }), "{carried:?}");
        adopt_carried_tests(&mut task, snapshot.planner_tests, &carried);
        assert_eq!(
            task.planner_tests.protected.as_ref().unwrap()["check.sh"],
            sha256_hex(b"new\n")
        );
        assert!(
            task.decisions
                .iter()
                .any(|d| d.contains("protected check.sh now matches the new base")),
            "{:?}",
            task.decisions
        );
        if let git::Rebase::Moved { new_sha, .. } = carried {
            task.base_sha = new_sha;
        }
        let changed = git::changed_files(&wt, task.work_base()).unwrap();
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert_eq!(
            std::fs::read_to_string(wt.join("check.sh")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn a_whitespace_only_reformat_of_a_protected_test_is_no_change() {
        let wt = tempfile::tempdir().unwrap();
        let wt = wt.path();
        init_git_repo(wt);
        let original = "fn t() { assert_eq!(f(1), \"a b\"); }\n";
        std::fs::write(wt.join("check.rs"), original).unwrap();
        std::fs::write(wt.join("check.py"), original).unwrap();
        git_in(wt, &["add", "."]);
        git_in(wt, &["commit", "-q", "-m", "tests"]);
        let rev = git::resolve_commit(wt, "HEAD").unwrap();
        let check =
            |path: &str, text: &str| reformatted_only(wt, &rev, path, Some(text.as_bytes()));
        let reformatted = "fn t() {\n    assert_eq!(f(1), \"a b\");\n}\n";
        assert!(check("check.rs", reformatted));
        assert!(
            !check("check.rs", original),
            "an unchanged file is no reformat"
        );
        assert!(!check("check.rs", "fn t() { assert_eq!(f(1), \"ab\"); }\n"));
        assert!(!check(
            "check.rs",
            "fn t() { assert_eq!(f(2), \"a b\"); }\n"
        ));
        assert!(
            !check("check.py", reformatted),
            "whitespace counts in Python"
        );
        assert!(!check("missing.rs", reformatted));
    }

    async fn prepared_with_format(
        setup_json: Option<&str>,
        content: &str,
    ) -> (Task, std::path::PathBuf, tempfile::TempDir) {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        if let Some(json) = setup_json {
            std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
            std::fs::write(repo.join(".sushiai/setup.json"), json).unwrap();
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["commit", "-q", "-m", "setup"]);
        }
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/format", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha;
        task.planner_tests.files = vec![test("tests/planner.txt", content)];
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        (task, wt, dir)
    }

    #[tokio::test]
    async fn planner_tests_are_formatted_by_the_repo_command_before_the_commit() {
        let json = r#"{"format": "for f; do sed 's/ *$//' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \nb \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a\nb\n");
        let sha = task.planner_tests.commit.clone().unwrap();
        let committed = git::blob_at(&wt, &sha, "tests/planner.txt").unwrap();
        assert_eq!(committed, b"a\nb\n");
        let hashes = task.planner_tests.protected.clone().unwrap();
        assert_eq!(hashes["tests/planner.txt"], sha256_hex(b"a\nb\n"));
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("formatted 1 planner test file(s)")));
    }

    #[tokio::test]
    async fn a_failing_format_command_commits_the_planner_tests_as_written() {
        let json = r#"{"format": ["sh", "-c", "echo mangled > \"$1\"; echo no formatter >&2; exit 2", "sh"]}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        let sha = task.planner_tests.commit.clone().unwrap();
        assert_eq!(
            git::blob_at(&wt, &sha, "tests/planner.txt").unwrap(),
            b"a  \n"
        );
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("format command failed on planner tests")
                && d.contains("committed as written")));
    }

    #[tokio::test]
    async fn without_a_format_command_the_planner_tests_stay_as_written() {
        let (task, wt, _dir) = prepared_with_format(None, "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        let sha = task.planner_tests.commit.clone().unwrap();
        assert_eq!(
            git::blob_at(&wt, &sha, "tests/planner.txt").unwrap(),
            b"a  \n"
        );
        assert!(!task.decisions.iter().any(|d| d.contains("format")));
    }

    #[test]
    fn the_planner_tests_commit_names_the_task_subject_once() {
        assert_eq!(
            planner_tests_title("fix: stats cover every host"),
            "test: planner tests for stats cover every host"
        );
        assert_eq!(
            planner_tests_title("feat(orch)!: copy settings"),
            "test: planner tests for copy settings"
        );
        assert_eq!(
            planner_tests_title("Settings: copy between hosts"),
            "test: planner tests for Settings: copy between hosts"
        );
    }
    /// A format command that rewrites `(a, b)` to `(a, b,)` (not a whitespace
    /// change) once the file `fmt-on` exists in the worktree.
    fn gated_format_json(rule: &str) -> String {
        let script = format!(
            "[ -e implementation.txt ] && echo seen >> ../saw.log; [ -e fmt-on ] || exit 0; [ -e touched.txt ] && echo fmt >> touched.txt; for f; do sed '{rule}' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"
        );
        serde_json::json!({ "format": script }).to_string()
    }

    struct Carry {
        app: Arc<App>,
        task: Task,
        wt: std::path::PathBuf,
        repo: std::path::PathBuf,
        base_ref: String,
        _dir: tempfile::TempDir,
    }

    async fn prepared_for_carry(setup_json: &str, content: &str) -> Carry {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
        init_git_repo(&repo);
        std::fs::write(repo.join(".sushiai/setup.json"), setup_json).unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "setup"]);
        std::fs::write(repo.join("touched.txt"), "orig\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "tracked"]);
        let base_ref = git::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/carry", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha;
        task.base_ref = Some(base_ref.clone());
        task.planner_tests.files = vec![test("tests/planner.txt", content)];
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        Carry {
            app,
            task,
            wt,
            repo,
            base_ref,
            _dir: dir,
        }
    }

    async fn land_on_moved_base(c: &mut Carry) {
        c.task.verify = vec!["false".to_string()];
        let run_dir = c._dir.path().join("runs");
        let _ = carry_and_check(
            &c.app,
            &mut c.task,
            None,
            &c.wt,
            &run_dir,
            &c.base_ref,
            "Another task",
            &[],
            &CancelToken::new(),
        )
        .await;
    }

    fn assert_planner_commit_holds(c: &Carry, content: &str, parent: &str) {
        let commit = c.task.planner_tests.commit.clone().unwrap();
        assert_eq!(
            git::blob_at(&c.wt, &commit, "tests/planner.txt").unwrap(),
            content.as_bytes()
        );
        assert_eq!(c.task.planner_tests.files[0].content, content);
        assert_eq!(
            c.task.planner_tests.protected.clone().unwrap()["tests/planner.txt"],
            sha256_hex(content.as_bytes())
        );
        assert_eq!(git_in(&c.wt, &["rev-parse", &format!("{commit}^")]), parent);
        assert_eq!(git::head_sha(&c.wt).unwrap(), commit);
    }

    #[tokio::test]
    async fn the_planner_commit_made_again_by_a_carry_is_formatted() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        assert_eq!(c.task.planner_tests.files[0].content, "call(a, b)\n");
        std::fs::write(c.wt.join("implementation.txt"), "work\n").unwrap();
        std::fs::write(c.repo.join("fmt-on"), "").unwrap();
        let new_base =
            git::commit_paths(&c.repo, &["fmt-on".to_string()], "chore: formatter", "base")
                .unwrap();
        land_on_moved_base(&mut c).await;
        assert_planner_commit_holds(&c, "call(a, b,)\n", &new_base);
        assert_eq!(
            std::fs::read_to_string(c.wt.join("implementation.txt")).unwrap(),
            "work\n",
            "the agent's work comes back"
        );
        assert!(
            !c._dir.path().join("saw.log").exists(),
            "the format command saw only the planner commit, not the agent's work"
        );
        assert!(
            git::changed_files(&c.wt, c.task.planner_tests.commit.as_deref().unwrap())
                .unwrap()
                .contains(&"implementation.txt".to_string())
        );
        assert!(c
            .task
            .decisions
            .iter()
            .any(|d| d.contains("formatted the planner tests again")));
    }

    #[tokio::test]
    async fn a_formatter_changed_on_the_new_base_reformats_the_protected_tests() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        std::fs::write(c.wt.join("fmt-on"), "").unwrap();
        std::fs::write(c.repo.join("fmt-on"), "").unwrap();
        let _ = git::commit_paths(&c.repo, &["fmt-on".to_string()], "chore: formatter", "base");
        land_on_moved_base(&mut c).await;
        let newer = gated_format_json("s/(a, b,)/(a, b, c,)/");
        std::fs::write(c.repo.join(".sushiai/setup.json"), newer).unwrap();
        let new_base = git::commit_paths(
            &c.repo,
            &[".sushiai/setup.json".to_string()],
            "chore: newer formatter",
            "base",
        )
        .unwrap();
        land_on_moved_base(&mut c).await;
        assert_planner_commit_holds(&c, "call(a, b, c,)\n", &new_base);
    }

    #[tokio::test]
    async fn a_carry_leaves_clean_planner_tests_and_their_commit_alone() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        std::fs::write(c.repo.join("OTHER.txt"), "x\n").unwrap();
        let new_base =
            git::commit_paths(&c.repo, &["OTHER.txt".to_string()], "chore: other", "base").unwrap();
        land_on_moved_base(&mut c).await;
        assert_planner_commit_holds(&c, "call(a, b)\n", &new_base);
        assert!(!c.task.decisions.iter().any(|d| d.contains("again")));
    }

    #[tokio::test]
    async fn an_attempts_edit_of_a_protected_test_is_not_folded_into_the_planner_commit() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        let planned = c.task.planner_tests.commit.clone().unwrap();
        std::fs::write(c.wt.join("tests/planner.txt"), "call(x, y)\n").unwrap();
        std::fs::write(c.repo.join("fmt-on"), "").unwrap();
        let _ = git::commit_paths(&c.repo, &["fmt-on".to_string()], "chore: formatter", "base");
        land_on_moved_base(&mut c).await;
        let commit = c.task.planner_tests.commit.clone().unwrap();
        assert_eq!(commit, planned, "the carry is skipped, the commit stays");
        assert_eq!(
            git::blob_at(&c.wt, &commit, "tests/planner.txt").unwrap(),
            b"call(a, b)\n"
        );
        assert!(!c.task.decisions.iter().any(|d| d.contains("again")));
    }

    #[tokio::test]
    async fn a_formatter_editing_other_tracked_files_does_not_cost_the_agent_its_work() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        std::fs::write(c.wt.join("touched.txt"), "agent\n").unwrap();
        std::fs::write(c.wt.join("new.txt"), "new\n").unwrap();
        std::fs::write(c.repo.join("fmt-on"), "").unwrap();
        let new_base =
            git::commit_paths(&c.repo, &["fmt-on".to_string()], "chore: formatter", "base")
                .unwrap();
        land_on_moved_base(&mut c).await;
        assert_planner_commit_holds(&c, "call(a, b,)\n", &new_base);
        let read = |f: &str| std::fs::read_to_string(c.wt.join(f)).unwrap();
        assert_eq!(read("touched.txt"), "agent\n");
        assert_eq!(read("new.txt"), "new\n");
        assert!(git::format_aside_commit(&c.wt, &c.task.id).is_none());
    }

    #[tokio::test]
    async fn work_left_aside_by_a_crash_is_put_back_and_never_overwritten() {
        let json = gated_format_json("s/(a, b)/(a, b,)/");
        let mut c = prepared_for_carry(&json, "call(a, b)\n").await;
        std::fs::write(c.wt.join("touched.txt"), "agent\n").unwrap();
        let aside = git::set_work_aside(&c.wt, &c.task.id).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(c.wt.join("touched.txt")).unwrap(),
            "orig\n"
        );
        // A second set-aside while the first is pending is refused.
        std::fs::write(c.wt.join("touched.txt"), "other\n").unwrap();
        assert!(git::set_work_aside(&c.wt, &c.task.id).is_err());
        assert_eq!(
            git::format_aside_commit(&c.wt, &c.task.id).as_deref(),
            Some(aside.as_str())
        );
        // The next loop start puts the work back.
        std::fs::write(c.wt.join("touched.txt"), "orig\n").unwrap();
        recover_format_aside(&mut c.task, &c.wt).await;
        assert_eq!(
            std::fs::read_to_string(c.wt.join("touched.txt")).unwrap(),
            "agent\n"
        );
        assert!(git::format_aside_commit(&c.wt, &c.task.id).is_none());
        assert!(c
            .task
            .decisions
            .iter()
            .any(|d| d.contains("interrupted format run")));
        // Nothing left: a second recovery is a no-op.
        recover_format_aside(&mut c.task, &c.wt).await;
    }
}
