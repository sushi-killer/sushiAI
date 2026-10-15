use super::process::Daemon;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The one wall-clock budget every wait in this suite shares: 180 s, or
/// `SUSHIAI_TEST_DEADLINE_SECS` when it is a positive whole number (unset, 0 or anything else
/// means 180 s, like `tests/helpers/deadline.cjs`). Read once per test process. A test that measures
/// timing scales its slow job and its budget from this value instead of using its own constant.
pub fn deadline() -> Duration {
    static DEADLINE: OnceLock<Duration> = OnceLock::new();
    *DEADLINE.get_or_init(|| {
        let secs = std::env::var("SUSHIAI_TEST_DEADLINE_SECS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        Duration::from_secs(secs.unwrap_or(180))
    })
}

/// Polls `check` every 50 ms until it returns `Some` or the deadline passes (`None`).
pub(super) fn poll_deadline<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if start.elapsed() >= deadline() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Polls `check` until it returns `Some`; panics naming `what` after the deadline.
pub fn wait_until<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    poll_deadline(check)
        .unwrap_or_else(|| panic!("timed out after {:?} waiting for {what}", deadline()))
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
        panic!("task {task_id} did not settle in time, last status: {status}: {last}");
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
            "task {task_id} did not reach the expected status in time, stuck at {status}: {last}"
        );
    })
}

pub fn settle(daemon: &Daemon, task_id: &str) -> serde_json::Value {
    poll_until(daemon, task_id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    })
}
