//! Stopping processes: a whole process group, and every process whose
//! executable lives inside an orchd-owned directory.

use crate::process::{executable as exe_path, list_pids, parent_pid};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// SIGTERM to a group, then SIGKILL, give it this long in between.
pub const GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

/// SIGTERM the process group, give it [`GRACE`] to empty, then SIGKILL what
/// is left and reap `child`. Safe after `child` exited: a group that is
/// already empty returns at once.
pub async fn kill_group(pgid: Option<i32>, child: &mut tokio::process::Child) {
    if let Some(pgid) = pgid {
        // SAFETY: plain signal syscalls on a pgid this process created.
        if unsafe { libc::killpg(pgid, libc::SIGTERM) } == 0 {
            let deadline = Instant::now() + GRACE;
            while Instant::now() < deadline {
                // Reaps the leader; a zombie leader keeps the group alive.
                let _ = child.try_wait();
                if unsafe { libc::killpg(pgid, 0) } != 0 {
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    let _ = child.wait().await;
}

/// Blocking. SIGTERM every process (but this one) whose canonical
/// executable path lies inside `dir`, wait up to [`GRACE`], SIGKILL any
/// that remain. Does nothing when `dir` cannot be canonicalized.
pub fn stop_processes_under(dir: &Path) {
    stop_under(dir, |_| true);
}

/// Commands running per directory. A sibling's detached daemon is an orphan
/// too while its test still uses it, so only the last command to end in a
/// directory sweeps orphans.
static RUNNING: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

/// One command running in a directory, counted until it is dropped.
pub struct Running(PathBuf);

pub fn running_in(dir: &Path) -> Running {
    let mut map = RUNNING.lock().unwrap();
    *map.get_or_insert_with(HashMap::new)
        .entry(dir.to_path_buf())
        .or_default() += 1;
    Running(dir.to_path_buf())
}

impl Running {
    /// Whether no other command runs in this directory now.
    // ponytail: two commands ending at once both see the other and skip the
    // sweep; the next command there, or removing the worktree, catches it.
    pub fn alone(&self) -> bool {
        RUNNING
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(&self.0))
            == Some(&1)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut map = RUNNING.lock().unwrap();
        if let Some(map) = map.as_mut() {
            if let Some(n) = map.get_mut(&self.0) {
                *n -= 1;
                if *n == 0 {
                    map.remove(&self.0);
                }
            }
        }
    }
}

/// Like [`stop_processes_under`], only for processes whose parent is PID 1:
/// what a finished check left behind, never a sibling check's live process.
// ponytail: an orphan reparented to a subreaper (not PID 1) is not caught; the full sweep at worktree removal still catches it.
pub fn stop_orphans_under(dir: &Path) {
    stop_under(dir, |pid| parent_pid(pid) == Some(1));
}

fn stop_under(dir: &Path, keep: impl Fn(i32) -> bool) {
    let Ok(dir) = dir.canonicalize() else {
        return;
    };
    let me = std::process::id() as i32;
    let inside = || -> Vec<i32> {
        list_pids()
            .into_iter()
            .filter(|&pid| pid > 0 && pid != me)
            .filter(|&pid| keep(pid))
            .filter(|&pid| {
                exe_path(pid).is_some_and(|exe| {
                    exe.starts_with(&dir) || exe.canonicalize().is_ok_and(|c| c.starts_with(&dir))
                })
            })
            .collect()
    };
    let pids = inside();
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        unsafe {
            libc::kill(*pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + GRACE;
    let mut left = inside();
    while !left.is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
        left = inside();
    }
    for pid in left {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

/// Test helpers: a long-lived process whose executable lives in a directory
/// the test picks. A copy of `/bin/sleep` does not do: on Apple silicon a
/// copied system binary is killed at exec, so the copy is of the running
/// test binary, started on [`tests::sleeper_helper`].
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};

    pub const ENV: &str = "SUSHIAI_TEST_SLEEPER";
    /// A file the helper writes once it runs (its executable is then in place).
    pub const READY_ENV: &str = "SUSHIAI_TEST_SLEEPER_READY";
    pub const ARGS: &str = "procs::tests::sleeper_helper --exact --nocapture";

    /// Copies the test binary to `dir/sleeper` (the test, not the check, puts
    /// it there, to avoid ETXTBSY).
    pub fn place(dir: &Path) -> PathBuf {
        let exe = dir.join("sleeper");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        exe
    }

    /// The file the detached helper writes once it runs.
    pub fn ready_file(pidfile: &Path) -> PathBuf {
        let mut name = pidfile.as_os_str().to_os_string();
        name.push(".ready");
        PathBuf::from(name)
    }

    /// The one wall-clock budget of a positive wait: 180 s, or
    /// `SUSHIAI_TEST_DEADLINE_SECS` when it is a positive whole number (the contract of
    /// `tests/common` and `tests/helpers/deadline.cjs`).
    pub fn deadline() -> std::time::Duration {
        let secs = std::env::var("SUSHIAI_TEST_DEADLINE_SECS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        std::time::Duration::from_secs(secs.unwrap_or(180))
    }

    /// Waits until `path` exists; panics naming it at the deadline.
    pub fn wait_for_file(path: &Path) {
        let end = std::time::Instant::now() + deadline();
        while !path.exists() {
            assert!(
                std::time::Instant::now() < end,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// A shell fragment starting `exe` detached into its own session, its
    /// pid written to `pidfile` and its readiness to [`ready_file`]; it returns at once.
    pub fn detach(exe: &Path, pidfile: &Path) -> String {
        format!(
            "{ENV}=1 {READY_ENV}='{ready}' perl -MPOSIX -e 'POSIX::setsid(); for (1..50) {{ exec {{$ARGV[0]}} @ARGV; select(undef, undef, undef, 0.02) }}' '{exe}' {ARGS} >/dev/null 2>&1 </dev/null & echo $! > '{pid}'",
            exe = exe.display(),
            pid = pidfile.display(),
            ready = ready_file(pidfile).display(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn alive(pid: i32) -> bool {
        crate::process::alive(pid)
    }

    /// The body of the helper process the tests start from a copy of the
    /// test binary; a plain test run skips it.
    #[test]
    fn sleeper_helper() {
        if std::env::var_os(testing::ENV).is_some() {
            if let Some(ready) = std::env::var_os(testing::READY_ENV) {
                std::fs::write(ready, "x").unwrap();
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    fn spawn_insider(dir: &Path) -> std::process::Child {
        let exe = testing::place(dir);
        for _ in 0..50 {
            let mut cmd = Command::new(&exe);
            cmd.args(testing::ARGS.split(' '))
                .env(testing::ENV, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match cmd.spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => return other.unwrap(),
            }
        }
        panic!("the helper stayed busy");
    }

    fn spawn_sleep(program: &Path) -> std::process::Child {
        Command::new(program)
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn stops_a_process_whose_executable_is_inside_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mut inside = spawn_insider(tmp.path());
        let pid = inside.id() as i32;
        assert!(alive(pid));
        stop_processes_under(tmp.path());
        let _ = inside.wait();
        assert!(!alive(pid));
    }

    #[test]
    fn stops_a_detached_orphan_inside_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = testing::place(tmp.path());
        let pidfile = tmp.path().join("pid");
        let status = Command::new("sh")
            .arg("-c")
            .arg(testing::detach(&exe, &pidfile))
            .status()
            .unwrap();
        assert!(status.success());
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The helper writes its ready file once it runs: its executable is in place then.
        testing::wait_for_file(&testing::ready_file(&pidfile));
        assert!(
            crate::process::executable(pid)
                .is_some_and(|e| e.starts_with(tmp.path().canonicalize().unwrap())),
            "the detached helper did not run from the directory"
        );
        let parent = crate::process::parent_pid(pid);
        if parent != Some(1) {
            eprintln!("skipped: the orphan's parent is {parent:?}, not PID 1 (subreaper)");
            stop_processes_under(tmp.path());
            return;
        }
        stop_orphans_under(tmp.path());
        assert!(!alive(pid), "the orphan survived");
    }

    #[test]
    fn orphan_sweep_spares_a_process_whose_parent_is_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let mut inside = spawn_insider(tmp.path());
        let pid = inside.id() as i32;
        stop_orphans_under(tmp.path());
        let still = alive(pid);
        let _ = inside.kill();
        let _ = inside.wait();
        assert!(still, "a process with a live parent was signalled");
    }

    #[test]
    fn leaves_a_process_whose_executable_is_outside_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mut outside = spawn_sleep(Path::new("/bin/sleep"));
        let pid = outside.id() as i32;
        stop_processes_under(tmp.path());
        let still = alive(pid);
        let _ = outside.kill();
        let _ = outside.wait();
        assert!(still, "a process outside the directory was signalled");
    }

    #[test]
    fn a_missing_directory_stops_nothing() {
        let mut outside = spawn_sleep(Path::new("/bin/sleep"));
        let pid = outside.id() as i32;
        stop_processes_under(Path::new("/no/such/orchd/dir"));
        let still = alive(pid);
        let _ = outside.kill();
        let _ = outside.wait();
        assert!(still);
    }

    #[test]
    fn removing_a_task_worktree_stops_its_processes_and_spares_others() {
        let repo = tempfile::tempdir().unwrap();
        let wt = repo.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        let mut inside = spawn_insider(&wt);
        let mut outside = spawn_sleep(Path::new("/bin/sleep"));
        let (inside_pid, outside_pid) = (inside.id() as i32, outside.id() as i32);
        crate::delivery::git::recovery::remove_task_worktree(repo.path(), &wt);
        let _ = inside.wait();
        let spared = alive(outside_pid);
        let _ = outside.kill();
        let _ = outside.wait();
        assert!(!alive(inside_pid), "the insider survived the removal");
        assert!(spared, "the outsider was signalled");
        assert!(!wt.exists());
    }

    #[test]
    fn only_the_last_command_in_a_directory_is_alone() {
        let dir = tempfile::tempdir().unwrap();
        let first = running_in(dir.path());
        assert!(first.alone());
        let second = running_in(dir.path());
        assert!(!first.alone() && !second.alone());
        drop(second);
        assert!(first.alone());
    }
}
