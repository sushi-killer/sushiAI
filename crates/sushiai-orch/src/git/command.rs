use crate::git::GitError;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub(crate) fn run(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    run_bytes(cwd, args).map(|stdout| String::from_utf8_lossy(&stdout).to_string())
}

pub(crate) fn run_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| GitError(format!("git {}: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Whether any process of group `pgid` is left.
fn group_alive(pgid: i32) -> bool {
    // SAFETY: signal 0 only tests that the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// Like [`run`], with stdin closed (nothing to prompt on), no terminal prompt
/// and a time limit. The child leads its own process group. On the limit the
/// group gets SIGTERM, then SIGKILL after `grace`; once no process of the
/// group is left, an `index.lock` the run created is removed, since nothing
/// can still hold it.
pub(crate) fn run_bounded(
    cwd: &Path,
    args: &[&str],
    limit: std::time::Duration,
    grace: std::time::Duration,
) -> Result<String, GitError> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let shown = args.join(" ");
    let started = std::time::SystemTime::now();
    let mut child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| GitError(format!("git {shown}: {e}")))?;
    let pgid = child.id() as i32;
    // Drain both pipes on threads so a chatty child cannot block on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let tick = std::time::Duration::from_millis(20);
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= limit => {
                // SAFETY: signals the group this call created.
                unsafe { libc::kill(-pgid, libc::SIGTERM) };
                let stop = std::time::Instant::now();
                while stop.elapsed() < grace && !matches!(child.try_wait(), Ok(Some(_))) {
                    std::thread::sleep(tick);
                }
                // SAFETY: as above.
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
                let _ = child.wait();
                let gone = std::time::Instant::now();
                while group_alive(pgid) && gone.elapsed() < grace {
                    std::thread::sleep(tick);
                }
                if !group_alive(pgid) {
                    remove_run_index_lock(cwd, started);
                }
                return Err(GitError(format!(
                    "git {shown} timed out after {}s",
                    limit.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(tick),
            Err(e) => return Err(GitError(format!("git {shown}: {e}"))),
        }
    };
    let (out, err) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    if !status.success() {
        return Err(GitError(format!(
            "git {shown} failed: {}",
            String::from_utf8_lossy(&err).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// Removes the worktree's `index.lock` when it appeared after `since`.
fn remove_run_index_lock(cwd: &Path, since: std::time::SystemTime) {
    let Ok(dir) = run(cwd, &["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let lock = PathBuf::from(dir.trim()).join("index.lock");
    let fresh = std::fs::metadata(&lock)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m >= since);
    if fresh {
        let _ = std::fs::remove_file(&lock);
    }
}

pub(crate) fn quiet_git() -> [&'static str; 8] {
    [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "user.name=orchd",
        "-c",
        "user.email=orchd@localhost",
        "-c",
        "commit.gpgsign=false",
    ]
}
