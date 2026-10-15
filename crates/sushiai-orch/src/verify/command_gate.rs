use crate::harness::environment::augmented_path;
use crate::task::{Check, Task};
use std::path::{Path, PathBuf};

/// Why `/bin/sh` cannot run `command` from `cwd`, or `None` when it can: the
/// one gate for every check created from text. Only the shell's own parse
/// (`sh -n`) and lookup (`command -v`, file existence) decide.
pub(crate) fn command_refusal(cwd: &Path, command: &str) -> Option<String> {
    let runs = |args: &[&str]| {
        let mut sh = std::process::Command::new("/bin/sh");
        sh.args(args)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // A missing directory must not make every command look unparsable.
        if cwd.is_dir() {
            sh.current_dir(cwd);
        }
        sh.status().is_ok_and(|s| s.success())
    };
    if !runs(&["-n", "-c", command]) {
        return Some("it does not parse as shell".to_string());
    }
    // Only a plain leading word is checked: a path may be created by the
    // task itself, and grouping/quoting is left to `sh -n` above.
    let word = command
        .split_whitespace()
        .find(|w| !w.contains('=') && !matches!(*w, "(" | "{" | "!"))?;
    if word.chars().any(|c| "'\"`$\\*?[]~<>|&;(){}#".contains(c)) {
        return None;
    }
    if word.starts_with('-') {
        return Some(format!("`{word}` is an option, not a program"));
    }
    if word.contains('/') {
        return (word.starts_with('/') && !Path::new(word).exists())
            .then(|| format!("`{word}` does not exist"));
    }
    (!runs(&["-c", "command -v \"$1\"", "sh", word]))
        .then(|| format!("`{word}` is not a program here"))
}

/// Drops the checks whose command the gate refuses, each with a decision
/// line; the criterion stays and review judges it.
pub(crate) fn gate_checks(
    cwd: &Path,
    checks: Vec<Check>,
    decisions: &mut Vec<String>,
) -> Vec<Check> {
    checks
        .into_iter()
        .filter(|c| match command_refusal(cwd, &c.run) {
            Some(why) if !c.run.trim().is_empty() => {
                decisions.push(no_check_line(c.criterion, &c.run, &why));
                false
            }
            _ => true,
        })
        .collect()
}

/// [`gate_checks`] for the one held-out check.
pub(crate) fn gate_held_out(
    cwd: &Path,
    check: Option<Check>,
    decisions: &mut Vec<String>,
) -> Option<Check> {
    let check = check?;
    match command_refusal(cwd, &check.run) {
        Some(why) if !check.run.trim().is_empty() => {
            // The held-out command never appears in a decision line.
            let label = crate::task::checks::commands::held_out_label(check.criterion);
            decisions.push(no_check_line(check.criterion, &label, &why));
            None
        }
        _ => Some(check),
    }
}

pub(crate) fn no_check_line(criterion: usize, run: &str, why: &str) -> String {
    format!(
        "Orchestrator: criterion {criterion} has no check: `{}` is not a runnable command ({why}); review judges it",
        run.trim()
    )
}

/// Where a task's checks are gated: its worktree while that exists, else the
/// repo checkout.
pub(crate) fn gate_cwd(task: &Task) -> PathBuf {
    [&task.worktree, &task.repo]
        .into_iter()
        .map(PathBuf::from)
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}
