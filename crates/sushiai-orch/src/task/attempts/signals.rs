use crate::task::Task;
use std::collections::BTreeSet;

/// The names of failing tests a check's output tail lists.
fn failing_lines(tail: &str) -> Vec<&str> {
    tail.lines()
        .map(str::trim)
        .filter(|l| {
            (l.starts_with("test ") && l.ends_with("FAILED"))
                || l.starts_with("not ok ")
                || l.starts_with("FAIL ")
                || l.starts_with("--- FAIL")
        })
        .collect()
}

/// File-looking words (`a/b.ts`) in the failing lines of a tail.
fn failing_paths(tail: &str) -> Vec<String> {
    const EXTS: [&str; 10] = [
        "rs", "ts", "tsx", "js", "cjs", "mjs", "py", "go", "rb", "java",
    ];
    failing_lines(tail)
        .into_iter()
        .flat_map(|l| l.split_whitespace())
        .map(|w| w.trim_matches(|c: char| ",;:()[]\"'".contains(c)))
        .filter(|w| {
            w.rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && EXTS.contains(&ext))
        })
        .map(|w| w.trim_start_matches("./").to_string())
        .collect()
}

/// Signals read from the check results of the task's latest attempt that ran
/// checks: `timeout`, `install` (the environment, not the code), `flaky` (a
/// different test fails in each run of one command on the same tree) and `outside_diff`
/// (failing tests live in files the attempt did not change). Sorted.
pub fn signals_of(task: &Task) -> Vec<String> {
    match task.attempts.iter().rposition(|a| !a.verify.is_empty()) {
        Some(at) => attempt_signals(task, at),
        None => Vec::new(),
    }
}

/// [`signals_of`] for the attempt at `at`; none when it ran no checks.
pub fn attempt_signals(task: &Task, at: usize) -> Vec<String> {
    let mut signals = BTreeSet::new();
    if task.attempts.get(at).is_none_or(|a| a.verify.is_empty()) {
        return Vec::new();
    }
    let attempt = &task.attempts[at];
    for outcome in attempt.verify.iter().filter(|o| o.code != Some(0)) {
        if outcome.load_timeout || outcome.code.is_none() {
            signals.insert("timeout");
        }
        if outcome.setup_failed {
            signals.insert("install");
        }
        let now = crate::policy::test_output::failing_tests(&outcome.tail);
        let before = task.attempts[..at]
            .iter()
            .rev()
            .flat_map(|a| a.verify.iter())
            .find(|o| o.command == outcome.command && o.code != Some(0));
        if let Some(before) = before {
            // Another tree is other code: its different failures are a change, not a flake.
            let same_tree = outcome.tree.is_some() && outcome.tree == before.tree;
            let before = crate::policy::test_output::failing_tests(&before.tail);
            if same_tree && !now.is_empty() && !before.is_empty() && now != before {
                signals.insert("flaky");
            }
        }
        let paths = failing_paths(&outcome.tail);
        if !paths.is_empty()
            && !paths.iter().any(|p| {
                attempt
                    .changed_files
                    .iter()
                    .any(|f| f.ends_with(p.as_str()) || p.ends_with(f.as_str()))
            })
        {
            signals.insert("outside_diff");
        }
    }
    signals.into_iter().map(str::to_string).collect()
}
