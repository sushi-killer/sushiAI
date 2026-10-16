use crate::policy::paths::glob_match;
use crate::task::checks::commands::{check_targets, command_segments, flat_command, is_assignment};
use crate::task::{CheckSource, SourcedCheck, Task};
use std::path::{Path, PathBuf};

/// Whether `command` is named in the owner's request.
fn named_in_request(request: Option<&str>, command: &str) -> bool {
    let command = flat_command(command);
    !command.is_empty() && request.is_some_and(|r| flat_command(r).contains(&command))
}

/// A plain file name: no glob, and an extension after a non-empty name.
fn is_plain_file(target: &str) -> bool {
    !target.contains(['*', '?', '['])
        && target
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(name, ext)| {
                !name.is_empty()
                    && !name.ends_with('.')
                    && ext.chars().any(|c| c.is_ascii_alphabetic())
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
}

/// Whether every part of `command` names only targets the planner made: a
/// planner test file, or a plain file that is absent on the base. A bare
/// word, `$(…)` or a backtick in it, or a part without a target (`go test
/// ./...`), fails.
fn made_by_planner(task: &Task, command: &str) -> bool {
    let command = flat_command(command);
    let absent = targets_absent_on_base(&task.repo, &task.base_sha, &command);
    let planner_made = |target: &String| {
        let target = target.strip_prefix("./").unwrap_or(target);
        task.planner_tests.files.iter().any(|f| f.path == target)
            || (is_plain_file(target) && absent.iter().any(|a| a == target))
    };
    let segments = command_segments(&command);
    !segments.is_empty()
        && segments.iter().all(|segment| {
            if segment.contains("$(") || segment.contains('`') {
                return false;
            }
            let targets = check_targets(segment);
            // Every word after the runner must be a flag or one of the
            // targets: a bare word may name a directory or a path of the base.
            let words = segment
                .split_whitespace()
                .skip_while(|word| is_assignment(word))
                .skip(1)
                .filter(|word| !word.starts_with('-'))
                .count();
            !targets.is_empty() && words == targets.len() && targets.iter().all(planner_made)
        })
}

/// The source of one command of a plan, ruled once when the plan is stored:
/// the owner's when the request names it, the repo's when it is a
/// whole-repo suite or a verify command that is not about files the planner
/// made, else the planner's.
pub(crate) fn classify_check_source(
    task: &Task,
    cwd: &Path,
    command: &str,
    in_verify: bool,
) -> CheckSource {
    if named_in_request(task.request.as_deref(), command) {
        CheckSource::Owner
    } else if whole_repo_suite(cwd, command) || (in_verify && !made_by_planner(task, command)) {
        CheckSource::Repo
    } else {
        CheckSource::Planner
    }
}

pub(crate) fn record_plan_check_sources(task: &mut Task, cwd: &Path) {
    let sources = task
        .planner_check_commands()
        .into_iter()
        .map(|(command, in_verify)| {
            let source = classify_check_source(task, cwd, &command, in_verify);
            SourcedCheck { command, source }
        })
        .collect();
    task.record_check_sources(sources);
}

/// Older tasks stored the commands of the plan as bare strings. The same
/// ruling as at plan time gives each its source, with the repo directory in
/// place of the worktree; what it cannot decide is the repo's.
pub(crate) fn migrate_check_sources(task: &mut Task) {
    if task.planner_tests.legacy_commands.is_empty() {
        return;
    }
    let legacy = task.planner_tests.legacy_commands.clone();
    let repo = PathBuf::from(&task.repo);
    let sources = task
        .planner_check_commands()
        .into_iter()
        .map(|(command, in_verify)| {
            let source = if legacy.contains(&command) {
                classify_check_source(task, &repo, &command, in_verify)
            } else {
                CheckSource::Repo
            };
            SourcedCheck { command, source }
        })
        .collect();
    crate::task::checks::sources::migrate_sources(task, sources);
}

/// Whether `command` runs a whole repository's suite: `npm run ci`, `cargo
/// test --workspace`, `make test`, a bare `pytest`, `go test ./...`, or an
/// `npm|pnpm|yarn run <script>` whose `<cwd>/package.json` script body is one
/// of these (one level, no further script expansion). Such a check is
/// finalVerify's job, not a per-attempt verify command. The command is split
/// on `&&`, `||` and `;`; one such segment is enough.
pub(crate) fn whole_repo_suite(cwd: &Path, command: &str) -> bool {
    let segments = |text: &str| -> Vec<String> {
        text.replace("&&", ";")
            .replace("||", ";")
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let words_of = |segment: &str| -> Vec<String> {
        segment
            .split_whitespace()
            .skip_while(|w| w.contains('=') && !w.starts_with('-'))
            .map(String::from)
            .collect()
    };
    let direct = |words: &[String]| -> bool {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let is_suite_word = |x: &str| matches!(x, "ci" | "test" | "check");
        match w.as_slice() {
            ["npm", "test"] => true,
            ["npm", "run", x] => is_suite_word(x),
            ["pnpm" | "yarn", x] => is_suite_word(x),
            ["pnpm" | "yarn", "run", x] => is_suite_word(x),
            ["make", x] => is_suite_word(x),
            ["pytest"] => true,
            ["go", "test", rest @ ..] => {
                rest.contains(&"./...")
                    && rest.iter().all(|a| *a == "./..." || a.starts_with('-'))
                    && !rest.contains(&"-run")
            }
            ["cargo", "test", rest @ ..] => {
                if rest.contains(&"--workspace") {
                    return true;
                }
                let before_args = rest.iter().take_while(|a| **a != "--");
                let mut scoped_or_filtered = false;
                for a in before_args {
                    let flag = a.split('=').next().unwrap_or(a);
                    if matches!(flag, "-p" | "--package" | "--test" | "--lib" | "--bin")
                        || !a.starts_with('-')
                    {
                        scoped_or_filtered = true;
                    }
                }
                !scoped_or_filtered
            }
            _ => false,
        }
    };
    segments(command).iter().any(|segment| {
        let words = words_of(segment);
        if direct(&words) {
            return true;
        }
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let script = match w.as_slice() {
            ["npm" | "pnpm" | "yarn", "run", name] => *name,
            _ => return false,
        };
        let Ok(text) = std::fs::read_to_string(cwd.join("package.json")) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        json.get("scripts")
            .and_then(|s| s.get(script))
            .and_then(|b| b.as_str())
            .is_some_and(|body| segments(body).iter().any(|s| direct(&words_of(s))))
    })
}

/// Whether `target` (a file, a directory or a glob) names something in
/// `files`, the paths of a tree.
pub(crate) fn target_in_tree(target: &str, files: &[String]) -> bool {
    let dir = format!("{}/", target.trim_end_matches('/'));
    files
        .iter()
        .any(|f| f == target || f.starts_with(&dir) || glob_match(target, f))
}

/// The targets of `command` that are not in the tree of `base_sha`. Read with
/// `git ls-tree`, because the scratch worktree of a base run is gone by now.
/// Empty when the tree cannot be read: nothing is known to be absent.
pub(crate) fn targets_absent_on_base(repo: &str, base_sha: &str, command: &str) -> Vec<String> {
    let targets = check_targets(command);
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(files) = crate::git::history::tree_files(Path::new(repo), base_sha) else {
        return Vec::new();
    };
    targets
        .into_iter()
        .filter(|t| !target_in_tree(t, &files))
        .collect()
}
