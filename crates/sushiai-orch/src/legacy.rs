//! The one cleanup left from the `orchd` daemon: a process of an older build may still hold
//! the data dir. It is stopped, and the files only it used are removed.

use std::path::Path;
use std::time::{Duration, Instant};

const STOP_WAIT: Duration = Duration::from_secs(10);
/// Files the old daemon wrote beside the data: its pid, its socket and its control token.
const FILES: [&str; 3] = ["orchd.pid", "orchd.sock", "control.token"];

/// Stops the process named by `<data>/orchd.pid` when it is alive and looks like an `orchd`
/// (SIGTERM, 10 s, then SIGKILL), then removes the old files. A pid that names anything else
/// is left alone: it was recycled by an unrelated program.
pub fn stop(data_dir: &Path) {
    let pid = std::fs::read_to_string(data_dir.join(FILES[0]))
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|pid| *pid > 1 && *pid != std::process::id() as i32);
    if let Some(pid) = pid {
        if alive(pid) && looks_like_orchd(pid) {
            terminate(pid);
        }
    }
    for name in FILES {
        let _ = std::fs::remove_file(data_dir.join(name));
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Whether `ps` names an executable containing `orchd`. When `ps` cannot answer, the answer
/// is no: killing a process we cannot identify is worse than leaving an old daemon running.
fn looks_like_orchd(pid: i32) -> bool {
    match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).contains("orchd"),
        _ => false,
    }
}

fn terminate(pid: i32) {
    // SAFETY: plain signals to a process this function identified above.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + STOP_WAIT;
    while alive(pid) && !reaped(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if alive(pid) && !reaped(pid) {
        // SAFETY: the group is signalled only when the process leads it, so the signal can
        // never reach this daemon's own group.
        unsafe {
            if libc::getpgid(pid) == pid {
                libc::killpg(pid, libc::SIGKILL);
            } else {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// A zombie has exited and only waits for its parent; it counts as stopped.
fn reaped(pid: i32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .map(|out| {
            let stat = String::from_utf8_lossy(&out.stdout);
            stat.trim().is_empty() || stat.trim().starts_with('Z')
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    /// A process whose executable is called `orchd`: a copy of `sleep`.
    fn fake_orchd(dir: &Path) -> Child {
        let exe = dir.join("orchd");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        Command::new(&exe).arg("60").spawn().unwrap()
    }

    #[test]
    fn a_live_orchd_is_stopped_and_its_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = fake_orchd(dir.path());
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        for name in FILES {
            std::fs::write(data.join(name), child.id().to_string()).unwrap();
        }
        stop(&data);
        // `stop` waits for the process to exit; `wait` reaps it and shows it did not survive.
        let status = child.wait().unwrap();
        assert!(!status.success(), "the process was signalled: {status:?}");
        for name in FILES {
            assert!(!data.join(name).exists(), "{name} remains");
        }
    }

    #[test]
    fn a_pid_of_another_program_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        std::fs::write(dir.path().join("orchd.pid"), child.id().to_string()).unwrap();
        stop(dir.path());
        assert!(alive(child.id() as i32), "an unrelated process was stopped");
        assert!(!dir.path().join("orchd.pid").exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn no_pid_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        stop(dir.path());
    }
}
