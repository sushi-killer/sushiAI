//! The planner's test files and the files the checks name: kept with the task
//! (content and hash), written into the worktree before the first attempt,
//! denied to the implementer and written again whenever an attempt changes
//! them. They are committed only in the final landing or pull request commit.

#[cfg(test)]
use crate::policy::settings::SandboxMode;
#[cfg(test)]
use crate::task::Check;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use crate::engine::graph::landing::Landing;
#[cfg(test)]
use crate::task::FailureKind;

use crate::clock::now_ms;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::git_in;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::engine::verification::baseline::baseline_checks;
use crate::engine::verification::baseline::needs_baseline;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::harness::environment::augmented_path;
use crate::policy::paths::is_test_path;
use crate::task::PlannedTest;
use crate::task::PlannerTests;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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

/// Commands that only look at a file's content.
const PROBES: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "test", "["];

/// The path-like words of each simple command of `command`, with whether that
/// command is a pure content probe.
fn check_words(command: &str) -> Vec<(String, bool)> {
    // A substitution can feed a file's content anywhere: nothing is a probe.
    let substituted = command.contains("$(") || command.contains('`') || command.contains("<(");
    let mut out = Vec::new();
    for segment in command.split([';', '|', '&', '\n']) {
        let words = command_words(segment);
        let probe = !substituted && PROBES.contains(&probe_head(segment).as_str());
        let patterns = if probe {
            pattern_files(segment)
        } else {
            Vec::new()
        };
        out.extend(words.into_iter().map(|w| {
            let probe = probe && !patterns.contains(&w);
            (w, probe)
        }));
    }
    out
}

/// The command a simple command runs: after `!`, environment assignments and
/// wrappers such as `env` and `command`, without a directory.
fn probe_head(segment: &str) -> String {
    const WRAPPERS: [&str; 6] = ["env", "command", "exec", "time", "nice", "nohup"];
    let mut wrapped = false;
    for token in segment.split_whitespace() {
        let token = token.trim_matches(|c| c == '"' || c == '\'');
        let name = token.rsplit('/').next().unwrap_or(token);
        if token == "!" {
            continue;
        }
        if WRAPPERS.contains(&name) {
            wrapped = true;
            continue;
        }
        let assignment = token.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if assignment || (wrapped && token.starts_with('-')) {
            continue;
        }
        return name.to_string();
    }
    String::new()
}

/// The files a grep-like command reads its patterns from (`-f FILE`,
/// `--file FILE`, `--file=FILE`, `-qxFf FILE`): their content is the expected
/// result, not the text searched.
fn pattern_files(segment: &str) -> Vec<String> {
    let clean = |t: &str| clean_test_path(t.trim_matches(|c| c == '"' || c == '\'')).ok();
    let mut out = Vec::new();
    let mut next_is_file = false;
    for token in segment.split_whitespace() {
        if std::mem::take(&mut next_is_file) {
            out.extend(clean(token));
        } else if token == "--file" {
            next_is_file = true;
        } else if let Some(value) = token.strip_prefix("--file=") {
            out.extend(clean(value));
        } else if token.starts_with('-') && !token.starts_with("--") {
            for (i, c) in token.char_indices().skip(1) {
                if c == 'f' {
                    let rest = &token[i + 1..];
                    if rest.is_empty() {
                        next_is_file = true;
                    } else {
                        out.extend(clean(rest));
                    }
                    break;
                }
                if c == 'e' {
                    break;
                }
            }
        }
    }
    out
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

/// Whether `prose` names `path` as a whole path (not as part of a longer one).
fn names_path(prose: &str, path: &str) -> bool {
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/');
    prose.match_indices(path).any(|(i, _)| {
        let before = prose[..i].chars().next_back();
        let after = prose[i + path.len()..].chars().next();
        !before.is_some_and(is_path_char) && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// What the task is asked to do, in words: its title, goal and criteria
/// without the `-- check: <command>` tail the criteria carry.
fn task_prose(task: &Task) -> String {
    let mut prose = format!("{}\n{}", task.title, task.goal);
    for c in &task.criteria {
        prose.push('\n');
        prose.push_str(c.split(" -- check:").next().unwrap_or(c));
    }
    prose
}

/// The files that are unchangeable for this task: the planner's files and
/// every file a visible check command names, except a document a check only
/// probes and a file the task's own words name as the thing to change (never
/// a test file, whatever the words say). The held-out command is not read,
/// so its files stay unnamed.
pub(super) fn protected_paths(task: &Task) -> Vec<String> {
    let mut paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    // A document a check only probes (grep, test -s ...) is not protected: the
    // task is asked to write it. A document a check compares against (a
    // golden file) is the expected result and stays protected.
    let planned = paths.clone();
    let mut named: BTreeMap<String, bool> = BTreeMap::new();
    for check in &task.checks {
        for (word, probe) in check_words(&check.run) {
            let only_probed = named.entry(word).or_insert(true);
            *only_probed &= probe;
        }
    }
    let prose = task_prose(task);
    paths.extend(
        named
            .into_iter()
            .filter(|(p, probe)| {
                planned.contains(p)
                    || !((*probe && is_document(p)) || (!is_test_path(p) && names_path(&prose, p)))
            })
            .map(|(p, _)| p),
    );
    paths.sort();
    paths.dedup();
    paths
}

/// [`protected_paths`] with their sha256: the planner's files as stored, every
/// other one as it is at `rev`.
pub(super) fn protected_hashes(
    worktree: &Path,
    rev: &str,
    task: &Task,
) -> BTreeMap<String, String> {
    protected_paths(task)
        .into_iter()
        .filter_map(|p| {
            let bytes = match task.planner_tests.files.iter().find(|t| t.path == p) {
                Some(t) => Some(t.content.clone().into_bytes()),
                None => crate::git::history::blob_at(worktree, rev, &p),
            };
            bytes.map(|bytes| (p, sha256_hex(&bytes)))
        })
        .collect()
}

/// A task an older build prepared holds its planner tests in a commit on its
/// branch. The commit is undone once: the files stay in the tree, the stored
/// content is written over them and the record is cleared. A branch that is
/// final (done, landed, with a pull request) is left as it is. The decision
/// line when the record was cleared.
fn undo_tests_commit(task: &mut Task, worktree: &Path) -> Result<Option<String>, String> {
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
pub(super) async fn repair_protection(task: &mut Task, worktree: &Path) {
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

/// Carries the agent's work onto the moved base. The planner's files are not
/// part of that work: they are taken out of the tree first, so a path the base
/// added since the task began cannot conflict, and written again after the
/// carry whatever it did.
pub(super) fn carry_with_planner_tests(
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

/// After a carry: the hashes of the files the checks name are those of the new
/// head, so a file the new base changed is not mistaken for the agent's edit
/// and put back to the old content.
fn refresh_protected(
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

/// Stores the planner-tests state a carry returned, with one decision line for
/// every protected file whose hash the new base changed.
pub(super) fn adopt_carried_tests(
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

/// Before the first attempt something is left to do: a check has no baseline,
/// or the planner's files are not formatted and hashed yet.
pub(super) fn needs_prepare(task: &Task) -> bool {
    let t = &task.planner_tests;
    needs_baseline(task)
        || (t.protected.is_none() && (!t.files.is_empty() || !task.checks.is_empty()))
}

/// Writes the planner's files into `worktree`; their repo-relative paths.
pub(super) fn write_files(worktree: &Path, files: &[PlannedTest]) -> Result<Vec<String>, String> {
    let root = worktree.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for t in files {
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
        |script| {
            crate::verify::command::build_verify_command(worktree, script, sandbox.as_ref(), "")
        },
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
    let mut files = task.planner_tests.files.clone();
    for t in &mut files {
        if let Ok(text) = std::fs::read_to_string(worktree.join(&t.path)) {
            if text != t.content && text.len() <= MAX_TEST_BYTES {
                t.content = text;
                changed += 1;
            }
        }
    }
    crate::task::checks::edits::replace_planner_files(task, files);
    (changed > 0).then(|| {
        format!(
            "Orchestrator: formatted {changed} planner test file(s) with the repo's format command"
        )
    })
}

fn format_failed(reason: &str) -> String {
    let first = reason.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let short: String = first.chars().take(120).collect();
    format!("Orchestrator: format command failed on planner tests ({short}), stored as written")
}

/// The steps before attempt 1, each idempotent: write the planner's tests,
/// format them with the repo's format command and store what it left, baseline
/// the checks with the tests in place, store a sha256 per protected file.
/// `false` when cancelled.
pub(super) async fn prepare_tests(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> bool {
    if !task.planner_tests.files.is_empty() && task.planner_tests.protected.is_none() {
        let (wt, files) = (worktree.to_path_buf(), task.planner_tests.files.clone());
        let written = tokio::task::spawn_blocking(move || write_files(&wt, &files))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        match written {
            Ok(paths) => {
                let rev = task.base_sha.clone();
                if let Some(line) =
                    format_planner_tests(app, task, worktree, &paths, &rev, cancel).await
                {
                    task.record_decision(line);
                }
                task.record_decision(format!(
                    "Orchestrator: wrote {} planner test file(s); they are kept with the task and put back before every check",
                    paths.len()
                ));
            }
            Err(e) => {
                task.record_decision(format!(
                    "Orchestrator: planner tests not written, dropped: {e}"
                ));
                crate::task::checks::edits::replace_planner_files(task, Vec::new());
            }
        }
        task.touch(now_ms());
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
    }
    if !baseline_checks(app, task_id, task, worktree, cancel).await {
        return false;
    }
    if task.planner_tests.protected.is_none()
        && (!task.planner_tests.files.is_empty() || !task.checks.is_empty())
    {
        let rev = task.base_sha.clone();
        let (wt, snapshot) = (worktree.to_path_buf(), task.clone());
        let hashes = tokio::task::spawn_blocking(move || protected_hashes(&wt, &rev, &snapshot))
            .await
            .unwrap_or_default();
        task.record_protected_tests(hashes);
    }
    task.touch(now_ms());
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    true
}

/// After an attempt (and after a carry, before the checks and the landing
/// commit): the protected files that changed or no longer match their stored
/// hash are put back. The planner's files are written from the store, the
/// others restored from the base. Returns the files it restored; `None` when
/// nothing protected was touched.
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
    async fn a_failed_landing_check_keeps_the_planner_files_and_hashes_for_retry() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/landing", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().planner_tests.files = vec![test("tests/planner.sh", "true\n")];
        let id = task.id.clone();
        let cancel = CancelToken::new();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &cancel).await);
        let old_hashes = task.planner_tests.protected.clone();
        std::fs::write(wt.join("implementation.txt"), "implementation\n").unwrap();
        std::fs::write(repo.join("FIXED.txt"), "fixed\n").unwrap();
        git_in(&repo, &["add", "FIXED.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "fix: base"]);
        let new_base = crate::git::history::head_sha(&repo).unwrap();
        task.fixture_data().verify = vec!["false".to_string()];
        let run_dir = dir.path().join("runs");
        let result =
            crate::engine::graph::carry::CarryRun::new(&app, &mut task, None, &run_dir, &cancel)
                .in_worktree(&wt)
                .onto(&base_ref, "Another task", &[])
                .await;
        assert!(matches!(
            result,
            Err(Landing::Failed {
                kind: FailureKind::Verify,
                ..
            })
        ));
        assert_eq!(task.base_sha, new_base);
        assert_eq!(
            crate::git::history::head_sha(&wt).unwrap(),
            new_base,
            "no commit of ours"
        );
        assert_eq!(task.planner_tests.protected, old_hashes);
        assert_eq!(
            std::fs::read_to_string(wt.join("tests/planner.sh")).unwrap(),
            "true\n"
        );
        let changed = task.own_diff().changed_files(&wt).unwrap();
        assert_eq!(changed, vec!["implementation.txt"]);
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert!(wt.join("FIXED.txt").exists());

        task.fixture_data().verify.clear();
        assert!(matches!(
            crate::engine::graph::carry::CarryRun::new(&app, &mut task, None, &run_dir, &cancel)
                .in_worktree(&wt)
                .onto(&base_ref, "Another task", &[])
                .await,
            Ok(false)
        ));
        assert_eq!(crate::git::history::head_sha(&wt).unwrap(), new_base);
        let pending = crate::git::changes::changed_files(&wt, &new_base).unwrap();
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
        task.fixture_data().criteria =
            vec!["Guide documents it -- check: grep -q parser docs/guide.md".into()];
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "grep -q parser docs/guide.md notes/readme.rst && run-tests checks/t.test".into(),
            baseline: None,
        }];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["checks/t.test"]);
    }

    #[test]
    fn a_document_a_check_compares_against_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("tests/golden/expected.md", "expected\n"),
            ("docs/guide.md", "old\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![
            Check {
                criterion: 0,
                run: "render-docs | diff - tests/golden/expected.md".into(),
                baseline: None,
            },
            Check {
                criterion: 0,
                run: "grep -q parser docs/guide.md && cmp tests/golden/expected.md out.md".into(),
                baseline: None,
            },
        ];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["tests/golden/expected.md"]);
    }

    #[test]
    fn a_document_read_as_a_pattern_file_or_through_a_substitution_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("tests/golden/expected.md", "expected\n"),
            ("docs/guide.md", "old\n"),
        ]);
        for run in [
            "render | grep -qxFf tests/golden/expected.md",
            "render | grep -f tests/golden/expected.md",
            "render | grep --file tests/golden/expected.md",
            "render | rg --file=tests/golden/expected.md",
            "[ \"$(render)\" = \"$(cat tests/golden/expected.md)\" ]",
            "grep -q \"$(cat tests/golden/expected.md)\" out.txt",
            "grep -q `cat tests/golden/expected.md` out.txt",
        ] {
            let mut task = task_with_status(TaskStatus::Running);
            task.fixture_data().checks = vec![Check {
                criterion: 0,
                run: run.into(),
                baseline: None,
            }];
            let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
            assert_eq!(kept, vec!["tests/golden/expected.md"], "{run}");
        }
        // The searched file of a plain probe stays writable.
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "grep -qE 'x' docs/guide.md".into(),
            baseline: None,
        }];
        assert!(protected_hashes(&repo, "HEAD", &task).is_empty());
    }

    #[tokio::test]
    async fn the_guard_restores_an_edited_test_and_reports_which_files() {
        let (_dir, repo) = repo_with(&[("t/a.test", "one\n"), ("t/b.test", "two\n")]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().planner_tests.files =
            vec![test("t/a.test", "one\n"), test("t/b.test", "two\n")];
        task.fixture_data().planner_tests.protected = Some(protected_hashes(&repo, "HEAD", &task));
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
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/carry", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().criteria = vec!["it works".into()];
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "sh check.sh".into(),
            baseline: None,
        }];
        task.fixture_data().planner_tests.protected =
            Some(protected_hashes(&wt, &task.base_sha, &task));
        // The base moves and changes the protected file; the task did not.
        std::fs::write(repo.join("check.sh"), "new\n").unwrap();
        git_in(&repo, &["commit", "-qam", "newer check"]);
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();
        let mut snapshot = task.clone();
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        adopt_carried_tests(&mut task, snapshot.planner_tests.clone(), &carried);
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
        if let crate::delivery::git::carry::Rebase::Moved { new_sha, .. } = carried {
            task.fixture_data().base_sha = new_sha;
        }
        let changed = task.own_diff().changed_files(&wt).unwrap();
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert_eq!(
            std::fs::read_to_string(wt.join("check.sh")).unwrap(),
            "new\n"
        );
    }

    fn check(run: &str) -> Check {
        Check {
            criterion: 0,
            run: run.into(),
            baseline: None,
        }
    }

    #[test]
    fn a_negated_or_wrapped_probe_still_counts_as_a_probe() {
        let (_dir, repo) = repo_with(&[("docs/a.md", "a\n"), ("tests/golden/expected.md", "g\n")]);
        for run in [
            "! grep -q X docs/a.md",
            "env A=b grep -q X docs/a.md",
            "FOO=1 /usr/bin/grep -q X docs/a.md",
            "env -i command grep -q X docs/a.md",
            "! test -s docs/a.md",
        ] {
            let mut task = task_with_status(TaskStatus::Running);
            task.fixture_data().checks = vec![check(run)];
            assert!(protected_hashes(&repo, "HEAD", &task).is_empty(), "{run}");
        }
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![check("! diff - tests/golden/expected.md")];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["tests/golden/expected.md"]);
    }

    #[test]
    fn a_file_the_task_is_asked_to_change_is_not_protected_but_tests_and_fixtures_are() {
        let (_dir, repo) = repo_with(&[
            ("prompts/orchestrator.yaml", "x\n"),
            ("prompts/other.yaml", "y\n"),
            ("scripts/c.sh", "true\n"),
            ("tests/t.sh", "true\n"),
            ("tests/x.test.cjs", "z\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().title = "Change the prompt".into();
        task.fixture_data().goal =
            "Replace a line in prompts/orchestrator.yaml and add a tool.".into();
        task.fixture_data().criteria = vec![
            "The prompt is updated -- check: sh scripts/c.sh prompts/other.yaml".into(),
            "tests/x.test.cjs passes".into(),
        ];
        task.fixture_data().planner_tests.files = vec![test("tests/t.sh", "true\n")];
        task.fixture_data().checks = vec![
            check("sh tests/t.sh prompts/orchestrator.yaml"),
            check("sh scripts/c.sh prompts/other.yaml"),
            check("node --test tests/x.test.cjs"),
        ];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(
            kept,
            vec![
                "prompts/other.yaml",
                "scripts/c.sh",
                "tests/t.sh",
                "tests/x.test.cjs"
            ]
        );
    }

    #[tokio::test]
    async fn an_older_tasks_stale_protected_entries_are_dropped_before_an_attempt() {
        let (_app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().goal = "Replace a line in prompts/orchestrator.yaml".into();
        task.fixture_data().checks = vec![check("sh tests/t.sh prompts/orchestrator.yaml")];
        task.fixture_data().planner_tests.protected = Some(
            [
                ("prompts/orchestrator.yaml".to_string(), "h1".to_string()),
                ("tests/t.sh".to_string(), "h2".to_string()),
            ]
            .into(),
        );
        repair_protection(&mut task, &repo).await;
        let kept: Vec<String> = task
            .planner_tests
            .protected
            .clone()
            .unwrap()
            .into_keys()
            .collect();
        assert_eq!(kept, vec!["tests/t.sh"]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("no longer protecting prompts/orchestrator.yaml")));
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
        let created =
            crate::git::worktree::create_worktree(&repo, "task/format", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.fixture_data().planner_tests.files = vec![test("tests/planner.txt", content)];
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        (task, wt, dir)
    }

    #[tokio::test]
    async fn planner_tests_are_formatted_by_the_repo_command_once_at_prepare() {
        let json = r#"{"format": "for f; do sed 's/ *$//' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \nb \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a\nb\n");
        assert!(task.planner_tests.commit.is_none());
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a\nb\n"
        );
        let hashes = task.planner_tests.protected.clone().unwrap();
        assert_eq!(hashes["tests/planner.txt"], sha256_hex(b"a\nb\n"));
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("formatted 1 planner test file(s)")));
    }

    #[tokio::test]
    async fn a_failing_format_command_stores_the_planner_tests_as_written() {
        let json = r#"{"format": ["sh", "-c", "echo mangled > \"$1\"; echo no formatter >&2; exit 2", "sh"]}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a  \n"
        );
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("format command failed on planner tests")
                && d.contains("stored as written")));
    }

    #[tokio::test]
    async fn without_a_format_command_the_planner_tests_stay_as_written() {
        let (task, wt, _dir) = prepared_with_format(None, "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a  \n"
        );
        assert!(!task.decisions.iter().any(|d| d.contains("format")));
    }

    struct Work {
        app: Arc<App>,
        task: Task,
        wt: std::path::PathBuf,
        repo: std::path::PathBuf,
        base_ref: String,
        _dir: tempfile::TempDir,
    }

    async fn prepared_work(files: Vec<PlannedTest>) -> Work {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/work", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().planner_tests.files = files;
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        Work {
            app,
            task,
            wt,
            repo,
            base_ref,
            _dir: dir,
        }
    }

    fn commit_in(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git_in(dir, &["add", "."]);
        git_in(dir, &["commit", "-q", "-m", "change"]);
        crate::git::history::head_sha(dir).unwrap()
    }

    #[tokio::test]
    async fn the_planner_tests_are_files_in_the_tree_never_a_commit() {
        let w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        assert!(w.task.planner_tests.commit.is_none());
        assert_eq!(
            crate::git::history::head_sha(&w.wt).unwrap(),
            w.task.base_sha
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            w.task.planner_tests.protected.clone().unwrap()["tests/planner.txt"],
            sha256_hex(b"expect\n")
        );
    }

    #[tokio::test]
    async fn an_implementer_commit_that_sweeps_the_planner_files_is_not_its_own_work() {
        let w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        git_in(&w.wt, &["add", "-A"]);
        git_in(&w.wt, &["commit", "-q", "-m", "agent commit"]);
        let own = w.task.own_diff().changed_files(&w.wt).unwrap();
        assert_eq!(
            own,
            vec!["impl.txt"],
            "planner files are in the commit but not in the diff"
        );
        let stat = w.task.own_diff().stat(&w.wt).unwrap();
        assert_eq!(stat.files, 1);
        let diff = w.task.own_diff().diff(&w.wt, 10_000).unwrap();
        assert!(
            diff.contains("impl.txt") && !diff.contains("planner.txt"),
            "{diff}"
        );
        // The guard writes the files over whatever the agent did to them.
        std::fs::write(w.wt.join("tests/planner.txt"), "weakened\n").unwrap();
        let restored = guard_protected(&w.task, &w.wt, &own).await.unwrap();
        assert_eq!(restored, vec!["tests/planner.txt"]);
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
    }

    #[tokio::test]
    async fn a_carry_onto_a_moved_base_works_with_the_planner_tests_present() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        let new_base = commit_in(&w.repo, "OTHER.txt", "other\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(&carried, crate::delivery::git::carry::Rebase::Moved { new_sha, .. } if *new_sha == new_base),
            "{carried:?}"
        );
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), new_base);
        let read = |f: &str| std::fs::read_to_string(w.wt.join(f)).unwrap();
        assert_eq!(read("tests/planner.txt"), "expect\n");
        assert_eq!(read("impl.txt"), "work\n");
        assert_eq!(read("OTHER.txt"), "other\n");
    }

    #[tokio::test]
    async fn a_planner_path_the_base_added_since_the_task_began_does_not_block_the_carry() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        std::fs::create_dir_all(w.repo.join("tests")).unwrap();
        let new_base = commit_in(&w.repo, "tests/planner.txt", "the base wrote this\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(&carried, crate::delivery::git::carry::Rebase::Moved { new_sha, .. } if *new_sha == new_base),
            "{carried:?}"
        );
        // The protected file is the planner's, not the base's.
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("impl.txt")).unwrap(),
            "work\n"
        );
        let changed = w.task.own_diff().changed_files(&w.wt).unwrap();
        assert!(guard_protected(&w.task, &w.wt, &changed).await.is_none());
    }

    #[tokio::test]
    async fn a_task_with_an_old_tests_commit_is_migrated_and_then_carries() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        // What an older build left: the planner files and the agent's work as commits.
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        w.task.fixture_data().planner_tests.commit =
            Some(crate::git::history::head_sha(&w.wt).unwrap());
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        let new_base = commit_in(&w.repo, "OTHER.txt", "other\n");

        repair_protection(&mut w.task, &w.wt).await;
        assert!(w.task.planner_tests.commit.is_none());
        assert_eq!(
            crate::git::history::head_sha(&w.wt).unwrap(),
            w.task.base_sha
        );
        assert!(w
            .task
            .decisions
            .iter()
            .any(|d| d.contains("no longer a commit of the branch")));

        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), new_base);
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("impl.txt")).unwrap(),
            "work\n"
        );
    }

    #[tokio::test]
    async fn a_carry_with_an_old_tests_commit_migrates_first() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        w.task.fixture_data().planner_tests.commit =
            Some(crate::git::history::head_sha(&w.wt).unwrap());
        commit_in(&w.repo, "OTHER.txt", "other\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        assert!(w.task.planner_tests.commit.is_none());
    }

    #[tokio::test]
    async fn a_finished_task_keeps_its_branch_when_loaded() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        let tests = crate::git::history::head_sha(&w.wt).unwrap();
        w.task.fixture_data().planner_tests.commit = Some(tests.clone());
        w.task.fixture_data().pr_url = Some("https://example.invalid/pr/1".into());
        repair_protection(&mut w.task, &w.wt).await;
        assert_eq!(w.task.planner_tests.commit.as_deref(), Some(tests.as_str()));
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), tests);
        let _ = &w.app;
    }

    #[test]
    fn the_base_with_the_planner_tests_is_a_commit_no_branch_holds() {
        let (_dir, repo) = repo_with(&[("a.txt", "a\n")]);
        let base = crate::git::history::head_sha(&repo).unwrap();
        let files = vec![("t/new.txt".to_string(), b"new\n".to_vec())];
        let over = crate::delivery::git::commit::commit_over(&repo, &base, &files).unwrap();
        assert_ne!(over, base);
        assert_eq!(
            crate::git::history::blob_at(&repo, &over, "t/new.txt").unwrap(),
            b"new\n"
        );
        assert_eq!(
            crate::git::history::blob_at(&repo, &over, "a.txt").unwrap(),
            b"a\n"
        );
        assert_eq!(
            crate::delivery::git::commit::commit_over(&repo, &base, &files).unwrap(),
            over
        );
        assert_eq!(crate::git::history::head_sha(&repo).unwrap(), base);
    }
}
