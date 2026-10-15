use crate::policy::paths::glob_match;
use crate::policy::paths::matches_any_protected;
use crate::policy::settings::ScopedCheck;

/// The entries that scope `command` in `repo`.
fn scoping_entries<'a>(
    scoped: &'a [ScopedCheck],
    repo: &str,
    command: &str,
) -> Vec<&'a ScopedCheck> {
    scoped
        .iter()
        .filter(|e| {
            !e.paths.is_empty()
                && e.repo.as_deref().is_none_or(|r| r == repo)
                && glob_match(&e.command, command.trim())
        })
        .collect()
}

/// The commands in `commands` that stay after `scoped` entries are applied
/// to `changed` (repo-relative files), and one decision line per skipped
/// command. A command an entry matches, where no changed file matches the
/// entry's paths, is skipped; an entry without paths never scopes.
pub(crate) fn filter_scoped(
    scoped: &[ScopedCheck],
    repo: &str,
    commands: &[String],
    changed: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut run = Vec::new();
    let mut lines = Vec::new();
    for command in commands {
        let entries = scoping_entries(scoped, repo, command);
        let touched = entries
            .iter()
            .any(|e| changed.iter().any(|f| matches_any_protected(f, &e.paths)));
        if entries.is_empty() || touched {
            run.push(command.clone());
            continue;
        }
        let mut paths: Vec<&str> = Vec::new();
        for e in &entries {
            for p in &e.paths {
                if !paths.contains(&p.as_str()) {
                    paths.push(p);
                }
            }
        }
        lines.push(format!(
            "Orchestrator: skipped {command}: no change under {}",
            paths.join(", ")
        ));
    }
    (run, lines)
}

/// Whether a planned path (a file or a directory) can hold a file `glob`
/// matches.
fn planned_path_in_scope(glob: &str, planned: &str) -> bool {
    let planned = planned.trim_start_matches("./").trim_end_matches('/');
    if planned.is_empty() || glob_match(glob, planned) {
        return true;
    }
    // A glob with no literal prefix (`*.html`) falls under no directory.
    let literal: &str = glob.split(['*', '?']).next().unwrap_or("");
    !literal.is_empty() && literal.starts_with(&format!("{planned}/"))
}

/// The base preflight has no diff, so `planned` (the task's `paths`) stands
/// in for it; it runs every command when `planned` is empty.
pub(crate) fn filter_scoped_planned(
    scoped: &[ScopedCheck],
    repo: &str,
    commands: &[String],
    planned: &[String],
) -> Vec<String> {
    if planned.is_empty() {
        return commands.to_vec();
    }
    commands
        .iter()
        .filter(|command| {
            let entries = scoping_entries(scoped, repo, command);
            entries.is_empty()
                || entries.iter().any(|e| {
                    e.paths
                        .iter()
                        .any(|g| planned.iter().any(|p| planned_path_in_scope(g, p)))
                })
        })
        .cloned()
        .collect()
}
