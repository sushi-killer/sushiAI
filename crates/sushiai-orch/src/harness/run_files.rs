use crate::task::Fingerprint;
use std::path::Path;
use std::path::PathBuf;

/// The files a file-backed run (an implement or plan attempt) leaves next to
/// its `events.jsonl`: the child's stdin, its raw stdout and stderr, the exit
/// code its wrapper writes last, the argv it was started with, and the token
/// its Stop hook presents (mode 0600). They let a restarted daemon find the
/// run again and finish it (see `recovery.rs`).
pub(crate) struct RunFiles {
    pub stdin: PathBuf,
    pub raw: PathBuf,
    pub stderr: PathBuf,
    pub exit: PathBuf,
    pub meta: PathBuf,
    pub token: PathBuf,
}

impl RunFiles {
    pub fn of(events_path: &Path) -> Self {
        let at = |ext: &str| events_path.with_extension(ext);
        RunFiles {
            stdin: at("stdin"),
            raw: at("raw"),
            stderr: at("stderr.log"),
            exit: at("exit"),
            meta: at("run.json"),
            token: at("token"),
        }
    }
}

/// What becomes of a run that was `running` when the daemon stopped.
pub(crate) enum RunFate {
    /// It ended or still runs: the attempt stays `running` and the task's
    /// loop follows the run (review and verify come after it as usual).
    Resume,
    /// Its result is lost: the attempt is interrupted.
    Interrupted,
}

/// `Resume` when the run's group lives or its exit file exists. The group is
/// asked first: the wrapper writes the exit file before its group can die, so
/// a run that ends between the two checks is seen as ended, never as lost.
/// (Checking the file first let a run finishing at that instant be
/// interrupted and requeued, a duplicate run.)
pub(crate) fn fate_of_run(exit: &Path, group_alive: impl FnOnce() -> bool) -> RunFate {
    let alive = group_alive();
    if alive || exit.exists() {
        RunFate::Resume
    } else {
        RunFate::Interrupted
    }
}

pub(crate) fn clear_interrupted_credentials(run_dir: &Path) {
    for file in ["key", "mcp.json", "b/mcp.json"] {
        let _ = std::fs::remove_file(run_dir.join(file));
    }
}

/// Removes the unredacted output files (`*.raw`, `*.stderr.log`) of a run
/// directory. Only a settled attempt that nothing can resume loses them.
pub(crate) fn remove_raw_output(run_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".raw") || name.ends_with(".stderr.log") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn cancelled_fingerprint_path(events_path: &Path) -> PathBuf {
    events_path.with_extension("fingerprint.json")
}

/// The fingerprint a cancelled nested run left next to its events file.
pub(crate) fn read_cancelled_fingerprint(events_path: &Path) -> Option<Fingerprint> {
    let path = cancelled_fingerprint_path(events_path);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&text).ok()
}

pub(crate) fn write_cancelled_fingerprint(events_path: &Path, fingerprint: &Fingerprint) {
    if let Ok(json) = serde_json::to_string(fingerprint) {
        let _ = std::fs::write(cancelled_fingerprint_path(events_path), json);
    }
}
