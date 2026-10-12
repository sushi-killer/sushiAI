//! Stopping processes: a whole process group, and every process whose
//! executable lives inside an orchd-owned directory.

use crate::process::{executable as exe_path, list_pids};
use std::path::Path;
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
    let Ok(dir) = dir.canonicalize() else {
        return;
    };
    let me = std::process::id() as i32;
    let inside = || -> Vec<i32> {
        list_pids()
            .into_iter()
            .filter(|&pid| pid > 0 && pid != me)
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
    pub const ARGS: &str = "procs::tests::sleeper_helper --exact --nocapture";

    /// Copies the test binary to `dir/sleeper` (the test, not the check, puts
    /// it there, to avoid ETXTBSY).
    pub fn place(dir: &Path) -> PathBuf {
        let exe = dir.join("sleeper");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        exe
    }

    /// A shell fragment starting `exe` detached into its own session, its
    /// pid written to `pidfile`; it returns at once.
    pub fn detach(exe: &Path, pidfile: &Path) -> String {
        format!(
            "{ENV}=1 perl -MPOSIX -e 'POSIX::setsid(); for (1..50) {{ exec {{$ARGV[0]}} @ARGV; select(undef, undef, undef, 0.02) }}' '{exe}' {ARGS} >/dev/null 2>&1 </dev/null & echo $! > '{pid}'",
            exe = exe.display(),
            pid = pidfile.display(),
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
        crate::git::remove_task_worktree(repo.path(), &wt);
        let _ = inside.wait();
        let spared = alive(outside_pid);
        let _ = outside.kill();
        let _ = outside.wait();
        assert!(!alive(inside_pid), "the insider survived the removal");
        assert!(spared, "the outsider was signalled");
        assert!(!wt.exists());
    }
}
