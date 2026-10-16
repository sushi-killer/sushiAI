use crate::policy::paths::is_test_path;
use crate::task::PlannedTest;
use crate::task::Task;
use std::path::Path;

use std::collections::BTreeMap;

const MAX_TEST_FILES: usize = 8;
pub(in crate::engine) const MAX_TEST_BYTES: usize = 64 * 1024;

/// The repo-relative form of a planned path, or why it is refused.
pub(in crate::engine) fn clean_test_path(raw: &str) -> Result<String, &'static str> {
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
pub(in crate::engine) fn keep_planned_tests(
    tests: &[PlannedTest],
) -> (Vec<PlannedTest>, Vec<String>) {
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

/// Shell words of `command` that could be a path: split on whitespace and
/// the shell's separators and quotes.
pub(super) fn command_words(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || ";|&()<>\"'`=,{}".contains(c))
        .filter(|w| !w.is_empty() && !w.starts_with('-'))
        .filter_map(|w| clean_test_path(w).ok())
        .collect()
}

/// Commands that only look at a file's content.
const PROBES: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "test", "["];

/// The path-like words of each simple command of `command`, with whether the
/// task must change that file: a document a probe reads, or a file a probe
/// asserts an absence in (`! grep`, `test ! -e`), which the base fails until
/// the file changes. A positive probe of any other file guards a fixture. The
/// pattern of a grep is not a file.
fn check_words(command: &str) -> Vec<(String, bool)> {
    // A substitution can feed a file's content anywhere: nothing is a probe.
    let substituted = command.contains("$(") || command.contains('`') || command.contains("<(");
    let mut out = Vec::new();
    for segment in command.split([';', '|', '&', '\n']) {
        let head = probe_head(segment);
        let probe = !substituted && PROBES.contains(&head.as_str());
        let pattern = probe.then(|| grep_pattern(&head, segment)).flatten();
        let words: Vec<String> = command_words(segment)
            .into_iter()
            .filter(|w| pattern.as_ref() != Some(w))
            .collect();
        let negated = segment.split_whitespace().any(|t| t == "!");
        let patterns = if probe {
            pattern_files(segment)
        } else {
            Vec::new()
        };
        out.extend(words.into_iter().map(|w| {
            let writable = probe && !patterns.contains(&w) && (negated || is_document(&w));
            (w, writable)
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

/// The pattern word of a grep or rg that takes it positionally (no `-e`, `-f`
/// or `--regexp`): the first word after the command that is not an option.
fn grep_pattern(head: &str, segment: &str) -> Option<String> {
    if !matches!(head, "grep" | "egrep" | "fgrep" | "rg") {
        return None;
    }
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    if tokens.iter().any(|t| {
        t.starts_with("--regexp") || t.starts_with("--file") || {
            t.starts_with('-') && !t.starts_with("--") && (t.contains('e') || t.contains('f'))
        }
    }) {
        return None;
    }
    let start = tokens
        .iter()
        .position(|t| t.trim_matches(|c| c == '"' || c == '\'').rsplit('/').next() == Some(head))?;
    tokens[start + 1..]
        .iter()
        .find(|t| !t.starts_with('-'))
        .and_then(|t| clean_test_path(t.trim_matches(|c| c == '"' || c == '\'')).ok())
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
pub(in crate::engine) fn protected_paths(task: &Task) -> Vec<String> {
    let mut paths: Vec<String> = task
        .planner_tests
        .files
        .iter()
        .map(|t| t.path.clone())
        .collect();
    // A file the task must change ([`check_words`]) is not protected: a document
    // a check probes, a file a check wants a match gone from or deleted. A file
    // a check compares against (a golden file), a fixture a check greps for a
    // match, and every test file stay protected.
    let planned = paths.clone();
    let mut named: BTreeMap<String, bool> = BTreeMap::new();
    for check in &task.checks {
        for (word, writable) in check_words(&check.run) {
            let only_writable = named.entry(word).or_insert(true);
            *only_writable &= writable;
        }
    }
    let prose = task_prose(task);
    paths.extend(
        named
            .into_iter()
            .filter(|(p, writable)| {
                planned.contains(p) || is_test_path(p) || !(*writable || names_path(&prose, p))
            })
            .map(|(p, _)| p),
    );
    paths.sort();
    paths.dedup();
    paths
}
