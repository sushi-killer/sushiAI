use crate::git::command::{run, run_bytes};
use crate::git::history::blob_at;
use crate::git::GitError;
use std::path::Path;
use std::process::Command;

/// `git status --porcelain -z --untracked-files=all`, one path per returned
/// entry. `-z` (NUL-terminated, no quoting/escaping) is safer to parse than
/// the default newline format; `--untracked-files=all` lists every file
/// inside an untracked directory instead of just the directory name, so an
/// agent-created directory's contents are all individually visible to the
/// protected-path check.
pub fn status_porcelain(cwd: &Path) -> Result<Vec<String>, GitError> {
    Ok(status_entries(cwd)?.into_iter().map(|(_, p)| p).collect())
}

/// `(XY status, path)` for every entry of `git status --porcelain -z
/// --untracked-files=all`.
pub fn status_entries(cwd: &Path) -> Result<Vec<(String, String)>, GitError> {
    let out = run(
        cwd,
        // Read-only: a background lease poll must never take index.lock
        // while the task's own commit runs.
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    let mut entries = Vec::new();
    let mut fields = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 3 {
            continue;
        }
        let status_code = &entry[0..2];
        entries.push((status_code.to_string(), entry[3..].to_string()));
        if status_code.starts_with('R') || status_code.starts_with('C') {
            // Rename/copy entries are followed by the origin path as a
            // second NUL-terminated field; consume and discard it.
            fields.next();
        }
    }
    Ok(entries)
}

/// Undoes what ran in `cwd` since `before` was taken with
/// [`status_entries`]: a tracked path that changed is restored from HEAD, an
/// untracked path that was not there is deleted. Never stashes: the stash
/// is shared by every worktree of the repository.
pub fn restore_status(cwd: &Path, before: &[(String, String)]) -> Result<(), GitError> {
    for (code, path) in status_entries(cwd)? {
        if before.iter().any(|(c, p)| *c == code && *p == path) {
            continue;
        }
        if code == "??" {
            let full = cwd.join(&path);
            let _ = std::fs::remove_file(&full);
            let mut dir = full.parent();
            while let Some(d) = dir.filter(|d| *d != cwd && d.starts_with(cwd)) {
                if std::fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        } else if run(cwd, &["checkout", "HEAD", "--", &path]).is_err() {
            // Added by the run: not in HEAD.
            let _ = run(cwd, &["rm", "-f", "--cached", "--", &path]);
            let _ = std::fs::remove_file(cwd.join(&path));
        }
    }
    Ok(())
}

pub fn status_short(cwd: &Path) -> Result<String, GitError> {
    run(cwd, &["status", "--short"])
}

/// Stages everything (new files too, ignored ones stay out), so a plain
/// `git diff <base>` shows the whole change.
pub fn stage_all(cwd: &Path) -> Result<(), GitError> {
    run(cwd, &["-c", "core.hooksPath=/dev/null", "add", "-A"]).map(|_| ())
}

/// The whole change since `base` as a binary-safe patch.
pub fn patch_since(cwd: &Path, base: &str) -> Result<Vec<u8>, GitError> {
    stage_all(cwd)?;
    let out = Command::new("git")
        .args(["diff", "--binary", base])
        .current_dir(cwd)
        .output()
        .map_err(|e| GitError(format!("git diff: {e}")))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Makes the worktree hold exactly `patch` on top of `base_sha`: its own
/// commits and edits are dropped first. Ignored files are left alone.
pub fn replace_work_with_patch(
    worktree: &Path,
    base_sha: &str,
    patch: &[u8],
) -> Result<(), GitError> {
    run(worktree, &["reset", "-q", "--hard", base_sha])?;
    run(worktree, &["clean", "-q", "-fd"])?;
    if patch.is_empty() {
        return Ok(());
    }
    let file = worktree.join(".git-best-of.patch");
    // A worktree's `.git` is a file: keep the patch out of the tree.
    let dir = run(worktree, &["rev-parse", "--git-dir"])?;
    let file = if dir.trim().is_empty() {
        file
    } else {
        worktree.join(dir.trim()).join("best-of.patch")
    };
    std::fs::write(&file, patch).map_err(|e| GitError(format!("write patch: {e}")))?;
    let applied = run(
        worktree,
        &[
            "apply",
            "--binary",
            file.to_str()
                .ok_or_else(|| GitError("non-utf8 path".into()))?,
        ],
    );
    let _ = std::fs::remove_file(&file);
    applied.map(|_| ())
}

/// Tree id of everything in the worktree that git would track: tracked and
/// untracked non-ignored files. Built through a temporary index, so the real
/// index and `git status` are left as they were.
pub fn worktree_tree(worktree: &Path) -> Result<String, GitError> {
    static INDEX_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "orchd-index-{}-{}-{}",
        std::process::id(),
        INDEX_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let result = (|| {
        use std::io::Write;
        use std::process::Stdio;

        let tmp_str = tmp.to_string_lossy().to_string();
        let git = |args: &[&str], input: Option<&[u8]>| -> Result<String, GitError> {
            let mut child = Command::new("git")
                .args(args)
                .current_dir(worktree)
                .env("GIT_INDEX_FILE", &tmp_str)
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
            let written = if let Some(input) = input {
                child.stdin.take().expect("piped stdin").write_all(input)
            } else {
                Ok(())
            };
            let out = child
                .wait_with_output()
                .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
            written.map_err(|e| GitError(format!("git {} stdin: {e}", args.join(" "))))?;
            if !out.status.success() {
                return Err(GitError(format!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        // Keep tracked paths (including staged ignored files), but rehash
        // their content without trusting the real index's cached file stats.
        let entries = run_bytes(worktree, &["ls-files", "--stage", "-z"])?;
        git(&["update-index", "-z", "--index-info"], Some(&entries))?;
        git(&["add", "-A"], None)?;
        git(&["write-tree"], None)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// `git diff --name-only <old> <new>` between two tree ids.
pub fn diff_trees_name_only(cwd: &Path, old: &str, new: &str) -> Result<Vec<String>, GitError> {
    let out = run(cwd, &["diff", "--name-only", old, new])?;
    Ok(out
        .lines()
        .map(|l| l.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Files that differ between two worktree trees because the *task* changed
/// them: a file counts when it differs from its own base in either tree, so
/// one that differs only because the base moved between the two (carried-in
/// commits) is left out.
pub fn own_changes_between(
    cwd: &Path,
    old_tree: &str,
    old_base: &str,
    now_tree: &str,
    now_base: &str,
) -> Result<Vec<String>, GitError> {
    let moved = diff_trees_name_only(cwd, old_tree, now_tree)?;
    if moved.is_empty() {
        return Ok(moved);
    }
    let mut own: std::collections::HashSet<String> = std::collections::HashSet::new();
    own.extend(diff_trees_name_only(cwd, old_base, old_tree)?);
    own.extend(diff_trees_name_only(cwd, now_base, now_tree)?);
    Ok(moved.into_iter().filter(|p| own.contains(p)).collect())
}

/// `git diff --name-only <base>`.
pub fn diff_name_only(cwd: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let out = run(cwd, &["--no-optional-locks", "diff", "--name-only", base])?;
    Ok(out
        .lines()
        .map(|l| l.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn diff_stat(cwd: &Path, base: &str) -> Result<String, GitError> {
    run(cwd, &["diff", "--stat", base])
}

/// `git diff <base>`, truncated to `max_chars` with a note (spec: review
/// diff capped at 60000 chars).
pub fn diff_full(cwd: &Path, base: &str, max_chars: usize) -> Result<String, GitError> {
    let out = run(cwd, &["diff", base])?;
    Ok(truncate_diff(out, max_chars))
}

/// `git diff <old> <new> -- <paths>` between two tree ids, truncated like
/// [`diff_full`]. No paths, no diff.
pub fn diff_trees(
    cwd: &Path,
    old: &str,
    new: &str,
    paths: &[String],
    max_chars: usize,
) -> Result<String, GitError> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec!["diff", old, new, "--"];
    args.extend(paths.iter().map(String::as_str));
    let out = run(cwd, &args)?;
    Ok(truncate_diff(out, max_chars))
}

/// [`diff_full`] without `exclude` (paths).
pub fn diff_full_without(
    cwd: &Path,
    base: &str,
    exclude: &[String],
    max_chars: usize,
) -> Result<String, GitError> {
    let specs: Vec<String> = exclude
        .iter()
        .map(|p| format!(":(exclude,literal){p}"))
        .collect();
    let mut args = vec!["diff", base, "--", "."];
    args.extend(specs.iter().map(String::as_str));
    Ok(truncate_diff(run(cwd, &args)?, max_chars))
}

fn truncate_diff(out: String, max_chars: usize) -> String {
    if out.len() <= max_chars {
        return out;
    }
    let mut end = max_chars;
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... [truncated, {} bytes total]",
        &out[..end],
        out.len()
    )
}

/// Changed files, combining working-tree changes and committed changes
/// since `base` (spec step 5: `git status --porcelain` + `git diff
/// --name-only <base>`), de-duplicated.
pub fn changed_files(cwd: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let mut files = status_porcelain(cwd)?;
    files.extend(diff_name_only(cwd, base)?);
    files.sort();
    files.dedup();
    Ok(files)
}

/// Takes `paths` out of the tree and the index so only the work of the
/// agent is left: a path that exists at `rev` is put back as it is there, any
/// other one is deleted.
pub fn drop_paths(cwd: &Path, rev: &str, paths: &[String]) -> Result<(), GitError> {
    let (tracked, new): (Vec<String>, Vec<String>) = paths
        .iter()
        .cloned()
        .partition(|p| blob_at(cwd, rev, p).is_some());
    if !tracked.is_empty() {
        restore_paths(cwd, rev, &tracked)?;
    }
    if !new.is_empty() {
        let mut rm = vec!["rm", "-q", "--cached", "--ignore-unmatch", "--"];
        rm.extend(new.iter().map(String::as_str));
        run(cwd, &rm)?;
        for p in &new {
            let _ = std::fs::remove_file(cwd.join(p));
        }
    }
    Ok(())
}

/// Puts `paths` back as they are at `rev`, in the tree and the index.
pub fn restore_paths(cwd: &Path, rev: &str, paths: &[String]) -> Result<(), GitError> {
    let mut args = vec!["checkout", "-q", rev, "--"];
    args.extend(paths.iter().map(String::as_str));
    run(cwd, &args).map(|_| ())
}

// -- report facts -------------------------------------------------------------

/// One `git diff --numstat` line: path, lines added, lines removed (`None`
/// for a binary file).
pub type NumstatEntry = (String, Option<u64>, Option<u64>);

/// `git diff --numstat <from> <to>`.
pub fn numstat(repo: &Path, from: &str, to: &str) -> Result<Vec<NumstatEntry>, GitError> {
    let out = run(repo, &["diff", "--numstat", from, to])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let (a, r, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some((path.to_string(), a.parse().ok(), r.parse().ok()))
        })
        .collect())
}

/// Sums numstat entries; a binary file (no line counts) is a file with no
/// lines.
pub fn fold_numstat(entries: &[NumstatEntry]) -> crate::task::attempt::DiffStat {
    crate::task::attempt::DiffStat {
        files: entries.len() as u32,
        added: entries.iter().map(|e| e.1.unwrap_or(0)).sum(),
        removed: entries.iter().map(|e| e.2.unwrap_or(0)).sum(),
    }
}

/// The stat of `git diff --numstat <from> <to>`.
pub fn range_stat(
    repo: &Path,
    from: &str,
    to: &str,
) -> Result<crate::task::attempt::DiffStat, GitError> {
    Ok(fold_numstat(&numstat(repo, from, to)?))
}

/// True when `rel` is a tracked file with unchanged content at HEAD of the
/// checkout at `dir`.
pub fn is_committed_clean(dir: &Path, rel: &str) -> bool {
    let tracked = run(dir, &["ls-files", "--error-unmatch", "--", rel]).is_ok();
    tracked
        && run(dir, &["status", "--porcelain", "--", rel])
            .map(|s| s.trim().is_empty())
            .unwrap_or(false)
}

/// True when `rel` is tracked by the checkout at `dir`.
pub fn is_tracked(dir: &Path, rel: &str) -> bool {
    run(dir, &["ls-files", "--error-unmatch", "--", rel]).is_ok()
}
