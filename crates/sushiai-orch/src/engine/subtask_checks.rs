use crate::task::checks::commands::is_assignment;
use crate::task::checks::commands::runs_ci_entry;
use std::path::Path;

pub(super) struct ScopedRun {
    pub commands: Vec<String>,
    pub decisions: Vec<String>,
    pub deferred: Vec<String>,
}

fn names_target(root: &Path, command: &str) -> bool {
    command.split_whitespace().any(|token| {
        let token = token.trim_matches(['\'', '"', ';', '(', ')']);
        ["-p", "--package", "--test", "--manifest-path"].contains(&token)
            || ["--package=", "--test=", "--manifest-path="]
                .iter()
                .any(|flag| token.starts_with(flag))
            || (token.starts_with("-p") && token.len() > 2 && !token.starts_with("--"))
            || (!token.starts_with('-')
                && !is_assignment(token)
                && (root.join(token).is_file() || Path::new(token).extension().is_some()))
    })
}

/// The commands a subtask runs. A full CI entry is replaced by the planner's
/// `check` and deferred to the parent, which runs the entry on the combined
/// tree. Without a `check` every command runs as drafted.
pub(super) fn scope(root: &Path, commands: &[String], check: Option<&str>) -> ScopedRun {
    let mut run = ScopedRun {
        commands: Vec::new(),
        decisions: Vec::new(),
        deferred: Vec::new(),
    };
    for command in commands {
        let replacement = match check {
            Some(check) if runs_ci_entry(command) && !names_target(root, command) => {
                run.decisions.push(format!("Orchestrator: subtask ran {check} instead of {command}; the parent runs {command} on the combined tree"));
                run.deferred.push(command.clone());
                check.to_string()
            }
            _ => command.clone(),
        };
        if !run.commands.contains(&replacement) {
            run.commands.push(replacement);
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_check_replaces_each_ci_entry_and_defers_it() {
        let root = tempfile::tempdir().unwrap();
        let commands: Vec<String> = vec!["cargo test --workspace".into(), "npm run ci".into()];
        let result = scope(root.path(), &commands, Some("cargo test -p package-a"));
        assert_eq!(result.commands, ["cargo test -p package-a"]);
        assert_eq!(result.deferred, commands);
        assert_eq!(result.decisions, [
            "Orchestrator: subtask ran cargo test -p package-a instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree",
            "Orchestrator: subtask ran cargo test -p package-a instead of npm run ci; the parent runs npm run ci on the combined tree",
        ]);
    }

    #[test]
    fn explicit_targets_and_non_ci_commands_run_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let commands: Vec<String> = [
            "cargo test -p x --test y",
            "cargo test --package=x",
            "cargo test -px",
            "cargo test --test x",
            "cargo test --manifest-path a/Cargo.toml",
            "npm test tests/foo.test.cjs",
            "npm run ci -- src/foo.js",
            "node --test",
            "true",
        ]
        .iter()
        .map(|c| (*c).into())
        .collect();
        let result = scope(root.path(), &commands, Some("cargo test -p package-a"));
        assert_eq!(result.commands, commands);
        assert!(result.decisions.is_empty() && result.deferred.is_empty());
    }

    #[test]
    fn without_a_check_the_ci_entry_runs_as_drafted() {
        let root = tempfile::tempdir().unwrap();
        let commands: Vec<String> = vec!["npm run ci".into()];
        let result = scope(root.path(), &commands, None);
        assert_eq!(result.commands, commands);
        assert!(result.decisions.is_empty() && result.deferred.is_empty());
    }
}
