use crate::git::command::{run, run_bytes, succeeds};
use std::path::Path;

/// `git check-ignore` reports true only when the main checkout actually
/// ignores the path (its own `.gitignore`, global excludes, ...) -- being
/// merely untracked isn't enough, or an ordinary WIP file someone forgot to
/// `git add` would get bootstrap-copied into every worktree too.
pub(crate) fn is_git_ignored(repo_root: &Path, rel_path: &str) -> bool {
    succeeds(repo_root, &["check-ignore", "--quiet", rel_path])
}

/// Append `rel_paths` (repo-root-relative) to the worktree's own
/// `info/exclude` (resolved via `git rev-parse --git-path`, since a
/// worktree's git-dir lives under the main checkout's `.git/worktrees/<name>`,
/// not `<worktree>/.git`). Best-effort: bootstrapping still succeeds even if
/// this fails, it just means those paths could show up as "changed" until
/// fixed by hand.
fn append_to_info_exclude(worktree: &Path, rel_paths: &[String]) -> std::io::Result<()> {
    let out = match run(worktree, &["rev-parse", "--git-path", "info/exclude"]) {
        Ok(out) => out,
        Err(_) => return Ok(()),
    };
    let exclude_path = worktree.join(out.trim());
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)?;
    use std::io::Write;
    for p in rel_paths {
        writeln!(f, "/{p}")?;
    }
    Ok(())
}

/// Gitignored files a new worktree needs but git does not carry. With a
/// `.worktreeinclude` in the main checkout (gitignore syntax) these are the
/// untracked files it matches that the checkout also ignores; without one,
/// `.env`, `.env.local` and `.claude/settings.local.json` when ignored.
fn include_candidates(main_root: &Path) -> Vec<String> {
    let list = main_root.join(".worktreeinclude");
    let found: Vec<String> = if list.is_file() {
        let exclude = format!("--exclude-from={}", list.display());
        match run_bytes(
            main_root,
            &["ls-files", "-z", "--others", "--ignored", &exclude],
        ) {
            Ok(out) => String::from_utf8_lossy(&out)
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    } else {
        [".env", ".env.local", ".claude/settings.local.json"]
            .map(str::to_string)
            .to_vec()
    };
    // Only ever copy files the main checkout itself ignores.
    found
        .into_iter()
        .filter(|rel| is_git_ignored(main_root, rel))
        .collect()
}

/// Copies the files [`include_candidates`] names into the worktree: regular
/// files only (a symlink is skipped), never over a file the worktree already
/// has. Every copied path is also recorded in the worktree's `info/exclude`,
/// so it never shows up as a "changed file" in the gate or gets swept into
/// `git add -A`.
pub fn bootstrap_worktree(main_root: &Path, worktree: &Path) -> std::io::Result<()> {
    let mut copied = Vec::new();
    for rel in include_candidates(main_root) {
        let src = main_root.join(&rel);
        let dst = worktree.join(&rel);
        let is_file = std::fs::symlink_metadata(&src).is_ok_and(|m| m.file_type().is_file());
        if !is_file || dst.symlink_metadata().is_ok() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
            // A tracked symlinked directory must not send the copy outside.
            let inside = match (parent.canonicalize(), worktree.canonicalize()) {
                (Ok(p), Ok(w)) => p.starts_with(w),
                _ => false,
            };
            if !inside {
                continue;
            }
        }
        std::fs::copy(&src, &dst)?;
        copied.push(rel);
    }
    if !copied.is_empty() {
        append_to_info_exclude(worktree, &copied)?;
    }
    Ok(())
}
