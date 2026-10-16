use crate::delivery::git::carry::wip_ref;
use crate::git::command::{quiet_git, run, run_bounded};
use crate::git::GitError;
use std::path::Path;
use std::process::Command;

/// How long the final commit may run before it is stopped.
const COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long a stopped commit gets to finish after SIGTERM before SIGKILL.
const TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// `git add -A && git commit -m "<title>" --trailer "Task-Id: <id>"
/// --trailer "Attempt: <n>"`.
/// `-c core.hooksPath=/dev/null` on both calls: this runs on the *host*, on
/// a commit an agent's own worktree may have staged, and an agent-written
/// `pre-commit`/`commit-msg` hook must never get to execute arbitrary code
/// there. The commit also never signs (no passphrase prompt), reads no stdin
/// and is stopped after [`COMMIT_TIMEOUT`] (`add -A` is not bounded: killing it mid-write is worse).
pub fn commit(cwd: &Path, title: &str, task_id: &str, attempt_n: u32) -> Result<(), GitError> {
    if committed_attempt(cwd, task_id, attempt_n)? {
        return Ok(());
    }
    run(cwd, &["-c", "core.hooksPath=/dev/null", "add", "-A"])?;
    run_bounded(
        cwd,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            title,
            "--trailer",
            &format!("Task-Id: {task_id}"),
            "--trailer",
            &format!("Attempt: {attempt_n}"),
        ],
        COMMIT_TIMEOUT,
        TERM_GRACE,
    )?;
    Ok(())
}

/// A clean HEAD carrying this exact delivery receipt survives a restart
/// between the commit and the task's persisted completion.
fn committed_attempt(cwd: &Path, task_id: &str, attempt_n: u32) -> Result<bool, GitError> {
    if !run(cwd, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(false);
    }
    let receipt = run(
        cwd,
        &[
            "log",
            "-1",
            "--format=%(trailers:key=Task-Id,valueonly)%x00%(trailers:key=Attempt,valueonly)",
        ],
    )?;
    let Some((task, attempt)) = receipt.split_once('\0') else {
        return Ok(false);
    };
    Ok(task.trim() == task_id && attempt.trim() == attempt_n.to_string())
}

/// A commit that no branch holds: `rev` with `files` (path, content) added.
/// Its date is fixed, so the same input gives the same sha. For a checkout of
/// the base with the planner's tests that must not be a commit of the task.
pub fn commit_over(
    repo: &Path,
    rev: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String, GitError> {
    use std::io::Write;
    use std::process::Stdio;

    static INDEX_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "orchd-over-{}-{}",
        std::process::id(),
        INDEX_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| {
        let git = |args: &[&str], input: Option<&[u8]>| -> Result<String, GitError> {
            let mut child = Command::new("git")
                .args(["-c", "user.name=orchd", "-c", "user.email=orchd@localhost"])
                .args(args)
                .current_dir(repo)
                .env("GIT_INDEX_FILE", &tmp)
                .env("GIT_AUTHOR_DATE", "1970-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "1970-01-01T00:00:00Z")
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
            if let Some(input) = input {
                child
                    .stdin
                    .take()
                    .expect("piped stdin")
                    .write_all(input)
                    .map_err(|e| GitError(format!("git {} stdin: {e}", args.join(" "))))?;
            }
            let out = child
                .wait_with_output()
                .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
            if !out.status.success() {
                return Err(GitError(format!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        git(&["read-tree", rev], None)?;
        for (path, content) in files {
            let blob = git(&["hash-object", "-w", "--stdin"], Some(content))?;
            git(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{path}"),
                ],
                None,
            )?;
        }
        let tree = git(&["write-tree"], None)?;
        git(
            &["commit-tree", &tree, "-p", rev, "-m", "orchd planner tests"],
            None,
        )
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Saves the worktree's uncommitted work (tracked and untracked, not
/// ignored) as a commit at [`wip_ref`] without moving the branch. `true`
/// when there was anything to save.
pub fn save_wip(worktree: &Path, task_id: &str) -> Result<bool, GitError> {
    if run(worktree, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(false);
    }
    let head = run(worktree, &["rev-parse", "HEAD"])?.trim().to_string();
    let quiet = quiet_git();
    run(worktree, &[&quiet[..], &["add", "-A"]].concat())?;
    let saved = (|| {
        run(
            worktree,
            &[
                &quiet[..],
                &["commit", "-q", "--no-verify", "-m", "orchd wip"],
            ]
            .concat(),
        )?;
        let wip = run(worktree, &["rev-parse", "HEAD"])?.trim().to_string();
        run(worktree, &["update-ref", &wip_ref(task_id), &wip])
    })();
    let _ = run(worktree, &["reset", "-q", "--soft", &head]);
    saved.map(|_| true)
}
