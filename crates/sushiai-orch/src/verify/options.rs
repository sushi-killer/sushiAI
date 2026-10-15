use crate::harness::environment::augmented_path;
use std::path::Path;

/// Planners sometimes put a check only a person or the reviewer can do into
/// `verify` ("screenshot the panel", "npm run x (only if ...)"); run as a
/// shell command it fails every attempt forever. An entry that doesn't
/// parse as shell, or whose program doesn't exist here, becomes a criterion
/// for the reviewer instead. Returns `(runnable, for_review)`.
pub(crate) fn split_verify_commands(cwd: &Path, commands: &[String]) -> (Vec<String>, Vec<String>) {
    let runs = |args: &[&str]| {
        std::process::Command::new("/bin/sh")
            .args(args)
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    commands.iter().cloned().partition(|cmd| {
        if !runs(&["-n", "-c", cmd]) {
            return false;
        }
        // Only a plain leading word is checked: a path may be created by
        // the task itself, and grouping/quoting is left to `sh -n` above.
        match cmd
            .split_whitespace()
            .find(|w| !w.contains('=') && !matches!(*w, "(" | "{" | "!"))
        {
            Some(p)
                if p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) =>
            {
                runs(&["-c", "command -v \"$1\"", "sh", p])
            }
            _ => true,
        }
    })
}

/// The package runner the repo's lockfile names: pnpm, yarn, bun, else npm.
fn package_runner(worktree: &Path) -> &'static str {
    let has = |name: &str| worktree.join(name).exists();
    if has("pnpm-lock.yaml") {
        "pnpm"
    } else if has("yarn.lock") {
        "yarn"
    } else if has("bun.lockb") || has("bun.lock") {
        "bun"
    } else {
        "npm"
    }
}

/// Suggested quick-reply options for the synthesized "no verification
/// command" question: each of the target repo's own `package.json` scripts,
/// as `<runner> run <script>` (the runner follows the lockfile), `test`-named
/// scripts ranked first (they're by far the most likely answer), then the
/// test command of each other toolchain the repo shows: `cargo test`,
/// `go test ./...`, `pytest`, `make test`. Empty (not an error) when nothing
/// is recognised -- the question still accepts free text.
pub(crate) fn verify_options(worktree: &Path) -> Vec<String> {
    let mut options: Vec<String> = Vec::new();
    let scripts = std::fs::read_to_string(worktree.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("scripts").and_then(|s| s.as_object()).cloned());
    if let Some(scripts) = scripts {
        let runner = package_runner(worktree);
        let mut names: Vec<&String> = scripts.keys().collect();
        names.sort_by_key(|k| (!k.to_ascii_lowercase().contains("test"), k.as_str()));
        options.extend(
            names
                .into_iter()
                .take(4)
                .map(|k| format!("{runner} run {k}")),
        );
    }
    let has = |name: &str| worktree.join(name).exists();
    if has("Cargo.toml") {
        options.push("cargo test".to_string());
    }
    if has("go.mod") {
        options.push("go test ./...".to_string());
    }
    if has("pyproject.toml") || has("pytest.ini") {
        options.push("pytest".to_string());
    }
    let make_test = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(worktree.join(f)).ok())
        .any(|text| {
            text.lines().any(|l| {
                l.strip_prefix("test")
                    .is_some_and(|r| r.trim_start().starts_with(':'))
            })
        });
    if make_test {
        options.push("make test".to_string());
    }
    options
}
