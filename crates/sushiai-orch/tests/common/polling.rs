use super::process::Daemon;
use super::protocol::log_tail;
use std::path::Path;

// The one implementation of every wait of the Rust tests: a deadline shared by all the
// waits of a test, a poll interval, absence checks.
#[path = "../../../test_wait.rs"]
mod test_wait;
pub(super) use test_wait::poll_deadline;
#[allow(unused_imports)] // each test binary uses a subset
pub use test_wait::{deadline, holds_for, wait_state, wait_until, ABSENCE_WINDOW};

/// The last lines of the daemon's log, for the panic of a wait that ran out.
pub fn daemon_log_tail(daemon: &Daemon) -> String {
    log_tail(&daemon.socket)
}

/// Polls the task `task_id` until `pick` returns `Some` for it. At the deadline the panic
/// prints the last task seen and the daemon log tail.
pub fn wait_task<T>(
    daemon: &Daemon,
    task_id: &str,
    what: &str,
    mut pick: impl FnMut(&serde_json::Value) -> Option<T>,
) -> T {
    wait_state(
        what,
        || {
            let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
            pick(&task).ok_or_else(|| format!("task {task_id}: {task}"))
        },
        || log_tail(&daemon.socket),
    )
}

/// Removes a directory tree that a stopped daemon or its agents may have just released:
/// success and NotFound return, ENOTEMPTY and EBUSY retry until the deadline, anything
/// else panics. Call it after the daemon was stopped and reaped.
pub fn remove_dir_retrying(path: impl AsRef<Path>) {
    let path = path.as_ref();
    let mut last = None;
    let removed = poll_deadline(|| match std::fs::remove_dir_all(path) {
        Ok(()) => Some(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(()),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTEMPTY | libc::EBUSY)) => {
            last = Some(e);
            None
        }
        Err(e) => panic!("removing {} failed: {e}", path.display()),
    });
    if removed.is_none() {
        panic!(
            "removing {} still failed at the deadline: {}",
            path.display(),
            last.map_or_else(String::new, |e| e.to_string())
        );
    }
}

pub fn poll_task_status(daemon: &Daemon, task_id: &str) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    poll_deadline(|| {
        let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
        let status = task["status"].as_str().unwrap_or("");
        if status != "queued" && status != "running" && status != "landing" {
            return Some(task);
        }
        last = task;
        None
    })
    .unwrap_or_else(|| {
        let status = last["status"].as_str().unwrap_or("");
        panic!(
            "task {task_id} did not settle in time, last status: {status}: {last}\n{}",
            log_tail(&daemon.socket)
        );
    })
}

pub fn poll_until(
    daemon: &Daemon,
    task_id: &str,
    pred: impl Fn(&str) -> bool,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    poll_deadline(|| {
        let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
        if pred(task["status"].as_str().unwrap_or("")) {
            return Some(task);
        }
        last = task;
        None
    })
    .unwrap_or_else(|| {
        let status = last["status"].as_str().unwrap_or("");
        panic!(
            "task {task_id} did not reach the expected status in time, stuck at {status}: {last}\n{}",
            log_tail(&daemon.socket)
        );
    })
}

pub fn settle(daemon: &Daemon, task_id: &str) -> serde_json::Value {
    poll_until(daemon, task_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    })
}
