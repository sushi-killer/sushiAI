use std::path::Path;

/// `true` while some process of group `pgid` is alive, not only its leader
/// (`kill(-pgid, 0)` signals every member).
pub(crate) fn group_alive(pgid: i32) -> bool {
    pgid > 0 && unsafe { libc::kill(-pgid, 0) == 0 }
}

/// `true` while group `pgid` is alive and is the run whose wrapper shell was
/// given `exit` as its `$0`. A pgid saved by an earlier daemon can have been
/// reused by a stranger's process since.
pub(crate) fn run_group_alive(pgid: i32, exit: &Path) -> bool {
    group_alive(pgid) && group_started_with(pgid, exit)
}

/// `true` when process `pgid` was started with `path` on its command line.
/// A run's wrapper shell carries its exit file as `$0`, so this tells the
/// run's group from an unrelated process that reuses a saved pgid.
pub(crate) fn group_started_with(pgid: i32, path: &Path) -> bool {
    crate::process::argv(pgid).is_some_and(|args| {
        args.iter()
            .any(|arg| arg.contains(&*path.to_string_lossy()))
    })
}

/// Best-effort: SIGTERM then (briefly later) SIGKILL a process group left
/// over from an unclean daemon shutdown, if it's still alive. Never panics
/// or blocks the caller for long -- this runs once at startup, before the
/// tokio runtime is doing anything else that matters.
#[cfg(unix)]
pub(crate) fn kill_stale_process_group(pgid: i32, run_dir: &Path) {
    unsafe {
        if libc::kill(pgid, 0) != 0 || !group_started_with(pgid, run_dir) {
            return; // gone, or a stranger's process that reused the pgid
        }
        libc::killpg(pgid, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe {
        if libc::kill(pgid, 0) == 0 {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_stale_process_group(_pgid: i32, _run_dir: &Path) {}
