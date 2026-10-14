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

/// [`protected_paths`] with their sha256 at `rev`.
pub(super) fn protected_hashes(
    worktree: &Path,
    rev: &str,
    task: &Task,
) -> BTreeMap<String, String> {
    protected_paths(task)
        .into_iter()
        .filter_map(|p| {
            git::blob_at(worktree, rev, &p).map(|bytes| {
                let hash = sha256_hex(&bytes);
                (p, hash)
            })
        })
        .collect()
}

/// The single planner tests commit on `base_sha`, rebuilt with `files`: the
/// agent's own commits are undone first (their changes stay in the tree, as
/// uncommitted work), and only the planner's paths go into the new commit, so
/// it keeps the shape the carry onto a moved base requires.
pub(super) fn rebuild_tests_commit(
    wt: &Path,
    base_sha: &str,
    files: &[PlannedTest],
    title: &str,
    task_id: &str,
) -> Result<String, String> {
    for f in files {
        let target = wt.join(&f.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, &f.content).map_err(|e| e.to_string())?;
    }
    git::uncommit_to(wt, base_sha).map_err(|e| e.to_string())?;
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    git::commit_paths(wt, &paths, &planner_tests_title(title), task_id).map_err(|e| e.to_string())
}

/// The recorded tests commit is not in the branch any more (something else
/// rebased or rewrote it): find the commit that carries the planner's files
/// now, and record it and its parent as the tests commit and the base.
/// `Some((commit, base))` when the record was changed.
pub(super) fn reanchor_tests_commit(
    task: &mut Task,
    worktree: &Path,
) -> Result<Option<(String, String)>, String> {
    let Some(recorded) = task.planner_tests.commit.clone() else {
        return Ok(None);
    };
    if task.planner_tests.files.is_empty()
        || git::is_ancestor(worktree, &recorded, "HEAD") == Some(true)
    {
        return Ok(None);
    }
    let holds_the_files = |c: &str| {
        task.planner_tests
            .files
            .iter()
            .all(|f| git::blob_at(worktree, c, &f.path).is_some_and(|b| b == f.content.as_bytes()))
    };
    let found = git::commits_with_task_id(worktree, &task.id)
        .into_iter()
        .find(|c| {
            git::resolve_commit(worktree, &format!("{c}^2")).is_err() && holds_the_files(c)
        })
        .ok_or_else(|| {
            format!(
                "the recorded tests commit {} is not in the branch and no commit there holds the planner tests",
                short_sha(&recorded)
            )
        })?;
    let base = git::resolve_commit(worktree, &format!("{found}^")).map_err(|e| e.to_string())?;
    task.planner_tests.protected = Some(protected_hashes(worktree, &found, task));
    task.planner_tests.commit = Some(found.clone());
    task.base_sha = base.clone();
    Ok(Some((found, base)))
}

/// A tests commit that is not the single commit on `base_sha` (the branch
/// holds more commits, or `base_sha` moved by a merge the branch already
/// contains) is rebuilt on `base_sha`, so the carry accepts it again. The new
/// commit, or `None` when the state is right or cannot be repaired.
pub(super) fn normalise_tests_commit(
    task: &mut Task,
    worktree: &Path,
) -> Result<Option<String>, String> {
    let Some(commit) = task
        .planner_tests
        .commit
        .clone()
        .filter(|c| *c != task.base_sha)
    else {
        return Ok(None);
    };
    let head = git::head_sha(worktree).map_err(|e| e.to_string())?;
    let parent = git::resolve_commit(worktree, &format!("{commit}^")).ok();
    if head == commit && parent.as_deref() == Some(task.base_sha.as_str()) {
        return Ok(None);
    }
    if git::is_ancestor(worktree, &task.base_sha, "HEAD") != Some(true) {
        return Err(format!(
            "the base {} is not in the branch history",
            short_sha(&task.base_sha)
        ));
    }
    let sha = rebuild_tests_commit(
        worktree,
        &task.base_sha,
        &task.planner_tests.files,
        &task.title,
        &task.id,
    )?;
    task.planner_tests.protected = Some(protected_hashes(worktree, &sha, task));
    task.planner_tests.commit = Some(sha.clone());
    Ok(Some(sha))
}

/// Before every attempt, for a task an older build prepared: the tests commit
/// is brought into the shape the carry needs, the planner files are run through
/// the repo's formatter once (a test the repo's own format check rejects would
/// fail forever, as the agent may not touch it), and files that are no longer
/// protected are dropped from the stored set.
pub(super) async fn repair_protection(
    app: &App,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) {
    let note = |task: &mut Task, line: String| {
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    };
    if task.planner_tests.commit.is_some() && !task.planner_tests.files.is_empty() {
        let (wt, mut snapshot) = (worktree.to_path_buf(), task.clone());
        let repaired = tokio::task::spawn_blocking(move || {
            let anchored = reanchor_tests_commit(&mut snapshot, &wt);
            let rebuilt = if anchored.is_ok() {
                normalise_tests_commit(&mut snapshot, &wt)
            } else {
                Ok(None)
            };
            (anchored, rebuilt, snapshot)
        })
        .await;
        if let Ok((anchored, rebuilt, snapshot)) = repaired {
            match &anchored {
                Ok(Some((commit, base))) => {
                    task.planner_tests = snapshot.planner_tests.clone();
                    task.base_sha = snapshot.base_sha.clone();
                    task.decisions.push(format!(
                        "Orchestrator: the branch was rebased outside orchd; the planner tests are now commit {} on base {}",
                        short_sha(commit),
                        short_sha(base)
                    ));
                }
                Err(e) => note(
                    task,
                    format!("Orchestrator: the planner tests commit was not repaired: {e}"),
                ),
                Ok(None) => {}
            }
            match rebuilt {
                Ok(Some(sha)) => {
                    task.planner_tests = snapshot.planner_tests;
                    task.decisions.push(format!(
                        "Orchestrator: rebuilt the planner tests commit on the task's base, now at {}",
                        short_sha(&sha)
                    ));
                }
                Err(e) => note(
                    task,
                    format!("Orchestrator: the planner tests commit was not rebuilt: {e}"),
                ),
                Ok(None) => {}
            }
        }
        if !task.planner_tests.format_checked && !cancel.is_cancelled() {
            let (wt, commit) = (worktree.to_path_buf(), task.planner_tests.commit.clone());
            let at_tests = tokio::task::spawn_blocking(move || git::head_sha(&wt).ok() == commit)
                .await
                .unwrap_or(false);
            if at_tests {
                reformat_carried_tests(app, task, worktree, cancel).await;
                task.planner_tests.format_checked = !cancel.is_cancelled();
            } else {
                note(
                    task,
                    "Orchestrator: the planner tests were not formatted: the branch is not at the tests commit".to_string(),
                );
            }
        }
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
            task.planner_tests.protected = Some(pruned);
            task.decisions.push(format!(
                "Orchestrator: no longer protecting {}: the task is asked to change it",
                dropped.join(", ")
            ));
        }
    }
}

/// Carries the task's work and its trusted test commit together, refreshing
/// the test anchor after the carry while keeping implementation uncommitted.
pub(super) fn carry_with_planner_tests(
    task: &mut Task,
    worktree: &Path,
    base_ref: &str,
) -> Result<git::Rebase, git::GitError> {
    let _ = normalise_tests_commit(task, worktree);
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
                task.planner_tests.format_checked = true;
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

    #[test]
    fn a_document_a_check_compares_against_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("tests/golden/expected.md", "expected\n"),
            ("docs/guide.md", "old\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.checks = vec![
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
            task.checks = vec![Check {
                criterion: 0,
                run: run.into(),
                baseline: None,
            }];
            let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
            assert_eq!(kept, vec!["tests/golden/expected.md"], "{run}");
        }
        // The searched file of a plain probe stays writable.
        let mut task = task_with_status(TaskStatus::Running);
        task.checks = vec![Check {
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
            task.checks = vec![check(run)];
            assert!(protected_hashes(&repo, "HEAD", &task).is_empty(), "{run}");
        }
        let mut task = task_with_status(TaskStatus::Running);
        task.checks = vec![check("! diff - tests/golden/expected.md")];
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
        task.title = "Change the prompt".into();
        task.goal = "Replace a line in prompts/orchestrator.yaml and add a tool.".into();
        task.criteria = vec![
            "The prompt is updated -- check: sh scripts/c.sh prompts/other.yaml".into(),
            "tests/x.test.cjs passes".into(),
        ];
        task.planner_tests.files = vec![test("tests/t.sh", "true\n")];
        task.checks = vec![
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
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.goal = "Replace a line in prompts/orchestrator.yaml".into();
        task.checks = vec![check("sh tests/t.sh prompts/orchestrator.yaml")];
        task.planner_tests.protected = Some(
            [
                ("prompts/orchestrator.yaml".to_string(), "h1".to_string()),
                ("tests/t.sh".to_string(), "h2".to_string()),
            ]
            .into(),
        );
        repair_protection(&app, &mut task, &repo, &CancelToken::new()).await;
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

    #[tokio::test]
    async fn an_unformatted_planner_file_from_an_older_task_is_formatted_and_recommitted_once() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
        init_git_repo(&repo);
        let json = r#"{"format": "for f; do sed 's/ *$//' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"}"#;
        std::fs::write(repo.join(".sushiai/setup.json"), json).unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "setup"]);
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/old", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha.clone();
        // What an older build committed: the planner file as written.
        task.planner_tests.files = vec![test("tests/planner.txt", "a  \nb \n")];
        std::fs::create_dir_all(wt.join("tests")).unwrap();
        std::fs::write(wt.join("tests/planner.txt"), "a  \nb \n").unwrap();
        let old = git::commit_paths(
            &wt,
            &["tests/planner.txt".to_string()],
            "test: planner",
            "id",
        )
        .unwrap();
        task.planner_tests.commit = Some(old.clone());
        task.planner_tests.protected = Some(protected_hashes(&wt, &old, &task));
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();

        repair_protection(&app, &mut task, &wt, &CancelToken::new()).await;
        let sha = task.planner_tests.commit.clone().unwrap();
        assert_ne!(sha, old);
        assert_eq!(
            git::blob_at(&wt, &sha, "tests/planner.txt").unwrap(),
            b"a\nb\n"
        );
        assert_eq!(
            task.planner_tests.protected.as_ref().unwrap()["tests/planner.txt"],
            sha256_hex(b"a\nb\n")
        );
        assert!(task.planner_tests.format_checked);
        assert_eq!(
            std::fs::read_to_string(wt.join("impl.txt")).unwrap(),
            "work\n"
        );
        // A second attempt formats nothing again.
        repair_protection(&app, &mut task, &wt, &CancelToken::new()).await;
        assert_eq!(task.planner_tests.commit.as_deref(), Some(sha.as_str()));
    }

    #[test]
    fn a_tests_commit_left_beyond_a_moved_base_is_rebuilt_so_the_carry_works() {
        let (_app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = git::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created = git::create_worktree(&repo, "task/merged", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = created.base_sha.clone();
        task.base_ref = Some(base_ref.clone());
        task.planner_tests.files = vec![test("t/a.test", "one\n")];
        std::fs::create_dir_all(wt.join("t")).unwrap();
        std::fs::write(wt.join("t/a.test"), "one\n").unwrap();
        let tests =
            git::commit_paths(&wt, &["t/a.test".to_string()], "test: planner", "id").unwrap();
        task.planner_tests.commit = Some(tests.clone());
        // The base moves and the branch merges it itself; the recorded base
        // follows (the "already contains" carry), the tests commit does not.
        std::fs::write(repo.join("moved.txt"), "moved\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "base moved"]);
        git_in(&wt, &["merge", "-q", "--no-edit", &base_ref]);
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();
        task.base_sha = git::resolve_commit(&repo, &base_ref).unwrap();

        let carried = carry_with_planner_tests(&mut task, &wt, &base_ref).unwrap();
        assert!(matches!(carried, git::Rebase::Unchanged), "{carried:?}");
        let rebuilt = task.planner_tests.commit.clone().unwrap();
        assert_ne!(rebuilt, tests);
        assert_eq!(git::head_sha(&wt).unwrap(), rebuilt);
        assert_eq!(
            git_in(&wt, &["rev-parse", &format!("{rebuilt}^")]),
            task.base_sha
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("impl.txt")).unwrap(),
            "work\n"
        );
        // The base moves on again: now the carry runs.
        std::fs::write(repo.join("moved2.txt"), "moved\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "base moved again"]);
        let carried = carry_with_planner_tests(&mut task, &wt, &base_ref).unwrap();
        assert!(matches!(carried, git::Rebase::Moved { .. }), "{carried:?}");
    }

    #[tokio::test]
    async fn a_branch_rebased_outside_orchd_gets_its_tests_commit_and_base_re_anchored_and_formatted(
    ) {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
        init_git_repo(&repo);
        let json = r#"{"format": "for f; do sed 's/ *$//' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"}"#;
        std::fs::write(repo.join(".sushiai/setup.json"), json).unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "setup"]);
        let main = git::branch_of(&repo, "HEAD").unwrap();
        // The recorded base sits on a side line; main has an older commit.
        git_in(&repo, &["branch", "side"]);
        std::fs::write(repo.join("older.txt"), "o\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "older main"]);
        let older_main = git::head_sha(&repo).unwrap();
        git_in(&repo, &["checkout", "-q", "side"]);
        std::fs::write(repo.join("side.txt"), "s\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "side base"]);
        let side = git::head_sha(&repo).unwrap();
        git_in(&repo, &["checkout", "-q", &main]);

        let wt = dir.path().join("task");
        git::create_worktree(&repo, "task/rebased", &wt, &side).unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = wt.to_string_lossy().to_string();
        task.base_sha = side.clone();
        task.base_ref = Some(main.clone());
        task.planner_tests.files = vec![test("tests/planner.txt", "a  \nb \n")];
        std::fs::create_dir_all(wt.join("tests")).unwrap();
        std::fs::write(wt.join("tests/planner.txt"), "a  \nb \n").unwrap();
        let recorded = git::commit_paths(
            &wt,
            &["tests/planner.txt".to_string()],
            "test: planner tests for x",
            &task.id,
        )
        .unwrap();
        task.planner_tests.commit = Some(recorded.clone());
        task.planner_tests.protected = Some(protected_hashes(&wt, &recorded, &task));
        // Something else rebases the branch onto main's older commit.
        git_in(&wt, &["rebase", "-q", "--onto", &older_main, &side]);
        let now = git::head_sha(&wt).unwrap();
        assert_ne!(now, recorded);
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();

        repair_protection(&app, &mut task, &wt, &CancelToken::new()).await;
        let sha = task.planner_tests.commit.clone().unwrap();
        assert_eq!(task.base_sha, older_main, "{:?}", task.decisions);
        assert_eq!(git::head_sha(&wt).unwrap(), sha);
        assert_eq!(git_in(&wt, &["rev-parse", &format!("{sha}^")]), older_main);
        assert_eq!(
            git::blob_at(&wt, &sha, "tests/planner.txt").unwrap(),
            b"a\nb\n"
        );
        assert_eq!(
            task.planner_tests.protected.as_ref().unwrap()["tests/planner.txt"],
            sha256_hex(b"a\nb\n")
        );
        assert!(task.planner_tests.format_checked);
        assert_eq!(
            std::fs::read_to_string(wt.join("impl.txt")).unwrap(),
            "work\n"
        );
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("rebased outside orchd")));
        // The carry onto main no longer skips.
        let mut snapshot = task.clone();
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &main).unwrap();
        assert!(
            matches!(carried, git::Rebase::Moved { .. } | git::Rebase::Unchanged),
            "{carried:?}"
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
