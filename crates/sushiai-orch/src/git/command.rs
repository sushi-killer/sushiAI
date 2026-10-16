use crate::git::GitError;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) fn run(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    run_bytes(cwd, args).map(|stdout| String::from_utf8_lossy(&stdout).to_string())
}

pub(crate) fn run_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    run_with(cwd, args, None, &[])
}

/// Like [`run_bytes`], with `stdin` piped in when given and `env` set on top
/// of the inherited environment.
pub(crate) fn run_with(
    cwd: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &OsStr)],
) -> Result<Vec<u8>, GitError> {
    use std::io::Write;
    let shown = args.join(" ");
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let mut child = cmd
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError(format!("git {shown}: {e}")))?;
    let written = match stdin {
        Some(input) => child.stdin.take().expect("piped stdin").write_all(input),
        None => Ok(()),
    };
    let out = child
        .wait_with_output()
        .map_err(|e| GitError(format!("git {shown}: {e}")))?;
    written.map_err(|e| GitError(format!("git {shown} stdin: {e}")))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git {shown} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// The exit code of `git <args>`; `None` when git could not be run or was
/// killed by a signal. Output is discarded.
pub(crate) fn exit_code(cwd: &Path, args: &[&str]) -> Option<i32> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?
        .status
        .code()
}

/// Whether `git <args>` ran and exited 0.
pub(crate) fn succeeds(cwd: &Path, args: &[&str]) -> bool {
    exit_code(cwd, args) == Some(0)
}

/// The non-empty lines of git's output, owned.
pub(crate) fn non_empty_lines(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// A scratch `GIT_INDEX_FILE` path in the temp dir that is deleted on drop,
/// so the real index and `git status` stay as they were.
pub(crate) struct TempIndex(PathBuf);

impl TempIndex {
    pub(crate) fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self(std::env::temp_dir().join(format!(
            "{prefix}-{}-{}-{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        )))
    }

    /// `git <args>` over this index, trimmed stdout.
    pub(crate) fn git(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &OsStr)],
    ) -> Result<String, GitError> {
        let mut all = vec![("GIT_INDEX_FILE", self.0.as_os_str())];
        all.extend_from_slice(env);
        let out = run_with(cwd, args, stdin, &all)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Whether any process of group `pgid` is left.
fn group_alive(pgid: i32) -> bool {
    // SAFETY: signal 0 only tests that the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// How a bounded run differs from the default.
pub(crate) struct Bounded<'a> {
    /// Environment set on top of `GIT_TERMINAL_PROMPT=0`.
    pub env: &'a [(&'a str, &'a str)],
    pub env_remove: &'a [&'a str],
    /// Remove an `index.lock` the run left behind when it is stopped.
    pub clean_index_lock: bool,
}

/// Why a bounded run gave no output.
pub(crate) enum BoundedError {
    /// Git could not be started or waited for (the io error).
    Io(String),
    TimedOut,
    /// Git exited non-zero; its stderr.
    Failed(String),
}

/// A drain thread reading one pipe; what it read so far stays readable after
/// the deadline even if a surviving process still holds the pipe open.
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Drain {
    fn new(pipe: Option<impl Read + Send + 'static>) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, done) = mpsc::channel();
        let sink = Arc::clone(&buf);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    sink.lock().unwrap().extend_from_slice(&chunk[..n]);
                }
            }
            let _ = tx.send(());
        });
        Self { buf, done }
    }

    /// Everything read once the pipe closed or `deadline` passed.
    fn collect(self, deadline: Instant) -> Vec<u8> {
        let _ = self
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        std::mem::take(&mut *self.buf.lock().unwrap())
    }
}

/// Like [`run`], with stdin closed (nothing to prompt on), no terminal prompt
/// and a time limit. The child leads its own process group. On the limit the
/// group gets SIGTERM, then SIGKILL after `grace`; once no process of the
/// group is left, an `index.lock` the run created is removed, since nothing
/// can still hold it. After the child exits its output is read for at most
/// `grace` more, so a grandchild holding a pipe cannot hang the caller.
pub(crate) fn run_bounded(
    cwd: &Path,
    args: &[&str],
    limit: Duration,
    grace: Duration,
) -> Result<String, GitError> {
    let opts = Bounded {
        env: &[],
        env_remove: &[],
        clean_index_lock: true,
    };
    let shown = args.join(" ");
    run_bounded_with(cwd, args, limit, grace, &opts).map_err(|e| match e {
        BoundedError::Io(e) => GitError(format!("git {shown}: {e}")),
        BoundedError::TimedOut => {
            GitError(format!("git {shown} timed out after {}s", limit.as_secs()))
        }
        BoundedError::Failed(stderr) => GitError(format!("git {shown} failed: {}", stderr.trim())),
    })
}

/// [`run_bounded`] with `opts`; the error keeps stderr apart for callers that
/// word it themselves.
pub(crate) fn run_bounded_with(
    cwd: &Path,
    args: &[&str],
    limit: Duration,
    grace: Duration,
    opts: &Bounded,
) -> Result<String, BoundedError> {
    use std::os::unix::process::CommandExt;
    let started = std::time::SystemTime::now();
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in opts.env {
        cmd.env(key, value);
    }
    for key in opts.env_remove {
        cmd.env_remove(key);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| BoundedError::Io(e.to_string()))?;
    let pgid = child.id() as i32;
    // Drain both pipes on threads so a chatty child cannot block on a full pipe.
    let out = Drain::new(child.stdout.take());
    let err = Drain::new(child.stderr.take());
    let (tx, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let status = match exited.recv_timeout(limit) {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            // Waiting failed: the child is out of our hands, stop its group.
            // SAFETY: signals the group this call created.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            return Err(BoundedError::Io(e.to_string()));
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // SAFETY: signals the group this call created.
            unsafe { libc::kill(-pgid, libc::SIGTERM) };
            let _ = exited.recv_timeout(grace);
            // SAFETY: as above.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            let _ = exited.recv();
            let gone = Instant::now();
            while group_alive(pgid) && gone.elapsed() < grace {
                std::thread::sleep(Duration::from_millis(20));
            }
            if opts.clean_index_lock && !group_alive(pgid) {
                remove_run_index_lock(cwd, started);
            }
            return Err(BoundedError::TimedOut);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // SAFETY: as above.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            return Err(BoundedError::Io("lost the process".into()));
        }
    };
    let deadline = Instant::now() + grace;
    let (out, err) = (out.collect(deadline), err.collect(deadline));
    if !status.success() {
        return Err(BoundedError::Failed(
            String::from_utf8_lossy(&err).to_string(),
        ));
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
