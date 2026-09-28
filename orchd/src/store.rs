//! JSON-file store: `<data>/settings.json`, `<data>/decisions.jsonl`,
//! `<data>/tasks/<id>/task.json`, `<data>/tasks/<id>/runs/<n>/*`,
//! `<data>/audits/<id>/{audit.json,report.json,brief.md,events.jsonl}`.
//! All writes to `task.json`/`settings.json` are atomic (write tmp file in
//! the same directory, then rename) so a crash never leaves a half-written
//! file for the next read.

use crate::model::{AttemptStatus, Audit, AuditReport, AuditStatus, Settings, Task, TaskStatus};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct Store {
    data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: impl Into<PathBuf>) -> io::Result<Self> {
        let data_dir = data_dir.into();
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(data_dir.join("tasks"))?;
        Ok(Store { data_dir })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn decisions_path(&self) -> PathBuf {
        self.data_dir.join("decisions.jsonl")
    }

    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("tasks").join(id)
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.task_dir(id).join("task.json")
    }

    pub fn run_dir(&self, id: &str, n: u32) -> PathBuf {
        self.task_dir(id).join("runs").join(n.to_string())
    }

    // -- settings --------------------------------------------------------

    pub fn load_settings(&self) -> io::Result<Settings> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json_atomic(&self.settings_path(), settings)
    }

    // -- tasks -------------------------------------------------------------

    pub fn save_task(&self, task: &Task) -> io::Result<()> {
        let dir = self.task_dir(&task.id);
        fs::create_dir_all(&dir)?;
        write_json_atomic(&self.task_path(&task.id), task)
    }

    pub fn load_task(&self, id: &str) -> io::Result<Option<Task>> {
        let path = self.task_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let task = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(task))
    }

    pub fn list_tasks(&self) -> io::Result<Vec<Task>> {
        let tasks_dir = self.data_dir.join("tasks");
        let mut out = Vec::new();
        if !tasks_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&tasks_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let task_json = entry.path().join("task.json");
            if !task_json.exists() {
                continue;
            }
            let text = fs::read_to_string(&task_json)?;
            match serde_json::from_str::<Task>(&text) {
                Ok(task) => out.push(task),
                Err(_) => continue, // skip corrupt entries rather than fail the whole listing
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    pub fn append_decision_line(&self, line: &str) -> io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.decisions_path())?;
        f.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        Ok(())
    }

    /// On daemon start: any attempt left `running` from a previous process
    /// means that process died mid-attempt. `killpg` its stored pgid first
    /// (if still alive -- the child was `setsid()`ed, so it survives the
    /// daemon's own death and would otherwise run forever unsupervised),
    /// delete its leftover per-run API key file if any, then mark the
    /// attempt `interrupted` and the task `stopped`, per spec step 10.
    /// Returns the tasks that were mutated.
    pub fn recover_interrupted(&self) -> io::Result<Vec<Task>> {
        let mut recovered = Vec::new();
        for mut task in self.list_tasks()? {
            let mut changed = false;
            for attempt in task.attempts.iter_mut() {
                if attempt.status == AttemptStatus::Running {
                    if let Some(pgid) = attempt.pgid {
                        kill_stale_process_group(pgid);
                    }
                    let key_file = self.run_dir(&task.id, attempt.n).join("key");
                    let _ = fs::remove_file(key_file);
                    attempt.status = AttemptStatus::Interrupted;
                    attempt.ended_at = Some(crate::model::now_ms());
                    changed = true;
                }
            }
            // A running attempt at startup means the daemon died under it (an
            // owner's stop marks its attempt interrupted right away), so the
            // task is queued again and resumes, rather than waiting for the
            // owner to notice and press Start.
            if changed {
                task.status = TaskStatus::Queued;
                task.updated_at = crate::model::now_ms();
                self.save_task(&task)?;
                recovered.push(task);
            }
        }
        Ok(recovered)
    }

    // -- audits ------------------------------------------------------------

    pub fn audit_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("audits").join(id)
    }

    pub fn save_audit(&self, audit: &Audit) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(&audit.id).join("audit.json"), audit)
    }

    pub fn load_audit(&self, id: &str) -> io::Result<Option<Audit>> {
        read_json(&self.audit_dir(id).join("audit.json"))
    }

    pub fn save_audit_report(&self, id: &str, report: &AuditReport) -> io::Result<()> {
        write_json_atomic(&self.audit_dir(id).join("report.json"), report)
    }

    pub fn load_audit_report(&self, id: &str) -> io::Result<Option<AuditReport>> {
        read_json(&self.audit_dir(id).join("report.json"))
    }

    /// Every audit of `repo`, newest first; corrupt entries are skipped.
    pub fn list_audits(&self, repo: &str) -> io::Result<Vec<Audit>> {
        let dir = self.data_dir.join("audits");
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Ok(Some(audit)) = read_json::<Audit>(&entry.path().join("audit.json")) {
                if audit.repo == repo {
                    out.push(audit);
                }
            }
        }
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(out)
    }

    /// On daemon start: an audit left `running` lost its run with the
    /// previous process, so it becomes `stopped`, and its per-run key file
    /// goes. Returns the audits that changed.
    pub fn recover_interrupted_audits(&self) -> io::Result<Vec<Audit>> {
        let dir = self.data_dir.join("audits");
        let mut recovered = Vec::new();
        if !dir.exists() {
            return Ok(recovered);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let Ok(Some(mut audit)) = read_json::<Audit>(&entry.path().join("audit.json")) else {
                continue;
            };
            if audit.status == AuditStatus::Running {
                let _ = fs::remove_file(entry.path().join("key"));
                audit.status = AuditStatus::Stopped;
                audit.error = Some("Interrupted by a daemon restart.".to_string());
                audit.ended_at = Some(crate::model::now_ms());
                self.save_audit(&audit)?;
                recovered.push(audit);
            }
        }
        Ok(recovered)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Best-effort: SIGTERM then (briefly later) SIGKILL a process group left
/// over from an unclean daemon shutdown, if it's still alive. Never panics
/// or blocks the caller for long -- this runs once at startup, before the
/// tokio runtime is doing anything else that matters.
#[cfg(unix)]
fn kill_stale_process_group(pgid: i32) {
    unsafe {
        if libc::kill(pgid, 0) != 0 {
            return; // already gone
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
fn kill_stale_process_group(_pgid: i32) {}

/// Write `value` as JSON to `path` atomically: serialize to a sibling temp
/// file, then rename over the destination. The rename is atomic on the same
/// filesystem, so a reader never observes a partially written file.
pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data.json");
    let tmp_path = dir.join(format!(".{}.tmp-{}", file_name, std::process::id()));
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Atomically write a secret-bearing file (the control token, a per-run API
/// key) restricted to owner read/write (mode 0600): `control.token` is
/// "rewritten each start", and per-run key files are deleted once their run
/// ends, so this is always a small, short-lived plain-text file, never JSON.
pub fn write_secret_file(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("secret");
    let tmp_path = dir.join(format!(".{}.tmp-{}", file_name, std::process::id()));
    {
        let mut f = fs::File::create(&tmp_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attempt, AttemptStatus, Harness, Stage, Tier};

    fn sample_task(id: &str, status: TaskStatus, attempt_status: AttemptStatus) -> Task {
        Task {
            id: id.to_string(),
            title: "Do thing".into(),
            goal: "Do the thing".into(),
            criteria: vec!["works".into()],
            verify: vec!["true".into()],
            final_verify: vec![],
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/do-thing".into(),
            base_sha: "deadbeef".into(),
            base_ref: None,
            depends_on: vec![],
            parent: None,
            status,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![Attempt {
                n: 1,
                stage: Stage::Implement,
                route_id: "claude-sonnet".into(),
                harness: Harness::Claude,
                model: "sonnet".into(),
                reason: "tier default".into(),
                session_id: Some("sess-1".into()),
                pgid: None,
                resumed: false,
                started_at: 1,
                ended_at: None,
                status: attempt_status,
                summary: None,
                handoff: None,
                changed_files: vec![],
                verify: vec![],
                gate_blocks: 0,
                prefix_tokens: None,
                review: None,
                failure: None,
                usage: None,
                cost_usd: None,
                review_cost_usd: None,
                advice: None,
            }],
            cost_usd: 0.0,
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn task_round_trips_through_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task = sample_task("t1", TaskStatus::Running, AttemptStatus::Running);
        store.save_task(&task).unwrap();
        let loaded = store.load_task("t1").unwrap().unwrap();
        assert_eq!(loaded.id, "t1");
        assert_eq!(loaded.branch, "task/do-thing");
        assert_eq!(loaded.attempts.len(), 1);
        assert!(store.load_task("missing").unwrap().is_none());
    }

    #[test]
    fn archived_flag_round_trips_and_defaults_false_for_a_pre_existing_task_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let mut task = sample_task("t1", TaskStatus::Done, AttemptStatus::Passed);
        task.archived = true;
        store.save_task(&task).unwrap();
        let loaded = store.load_task("t1").unwrap().unwrap();
        assert!(loaded.archived);

        // A task.json written before `archived` existed has no such key at
        // all -- simulate that directly rather than via `Task`, which would
        // always serialize the field.
        let old_dir = store.task_dir("old");
        fs::create_dir_all(&old_dir).unwrap();
        let mut value =
            serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed))
                .unwrap();
        value.as_object_mut().unwrap().remove("archived");
        fs::write(
            old_dir.join("task.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        let old_loaded = store.load_task("old").unwrap().unwrap();
        assert!(!old_loaded.archived);
    }

    /// A task.json written with fields since removed from `Variant` and
    /// `Attempt` must still load: serde ignores unknown fields by default,
    /// so dropping them from the structs is not a breaking change for a
    /// task already on disk.
    #[test]
    fn a_task_json_with_fields_since_removed_from_variant_and_attempt_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task_dir = store.task_dir("old");
        fs::create_dir_all(&task_dir).unwrap();
        let mut value =
            serde_json::to_value(sample_task("old", TaskStatus::Done, AttemptStatus::Passed))
                .unwrap();
        // A flag removed from `Variant` (2026-09-28) must not break loading.
        value["variant"]["leanContext"] = serde_json::json!(true);
        value["attempts"][0]["skills"] = serde_json::json!(["deslop"]);
        fs::write(
            task_dir.join("task.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        let loaded = store.load_task("old").unwrap().unwrap();
        assert_eq!(loaded.id, "old");
        assert_eq!(loaded.attempts.len(), 1);
    }

    #[test]
    fn settings_round_trips_and_defaults_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let defaulted = store.load_settings().unwrap();
        assert_eq!(defaulted.max_attempts, 4);

        let custom = Settings {
            max_attempts: 7,
            ..Settings::default()
        };
        store.save_settings(&custom).unwrap();
        let loaded = store.load_settings().unwrap();
        assert_eq!(loaded.max_attempts, 7);
    }

    #[test]
    fn list_tasks_sorts_newest_first_and_skips_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let mut older = sample_task("older", TaskStatus::Done, AttemptStatus::Passed);
        older.created_at = 1;
        let mut newer = sample_task("newer", TaskStatus::Done, AttemptStatus::Passed);
        newer.created_at = 2;
        store.save_task(&older).unwrap();
        store.save_task(&newer).unwrap();

        // A corrupt task directory should be skipped, not fail the listing.
        let corrupt_dir = store.task_dir("corrupt");
        fs::create_dir_all(&corrupt_dir).unwrap();
        fs::write(corrupt_dir.join("task.json"), b"not json").unwrap();

        let listed = store.list_tasks().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "newer");
        assert_eq!(listed[1].id, "older");
    }

    #[test]
    fn restart_recovery_marks_running_attempts_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let running_task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
        let done_task = sample_task("done", TaskStatus::Done, AttemptStatus::Passed);
        store.save_task(&running_task).unwrap();
        store.save_task(&done_task).unwrap();

        let recovered = store.recover_interrupted().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "running");

        let reloaded = store.load_task("running").unwrap().unwrap();
        assert_eq!(reloaded.status, TaskStatus::Queued);
        assert_eq!(reloaded.attempts[0].status, AttemptStatus::Interrupted);

        // Untouched task stays as-is.
        let untouched = store.load_task("done").unwrap().unwrap();
        assert_eq!(untouched.status, TaskStatus::Done);
    }

    #[test]
    fn recover_interrupted_kills_a_stale_process_group_if_still_alive() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        // Spawn a detached child in its own process group, the same way the
        // engine spawns harness/verify children, to prove recovery kills it
        // rather than just marking the attempt interrupted and leaving an
        // orphaned process running forever.
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().unwrap();
        let pgid = child.id() as i32;

        let mut task = sample_task("running", TaskStatus::Running, AttemptStatus::Running);
        task.attempts[0].pgid = Some(pgid);
        store.save_task(&task).unwrap();

        store.recover_interrupted().unwrap();

        // `try_wait()` (not a raw `kill(pid, 0)`) is the correct liveness
        // check here: once SIGTERM/SIGKILL lands, the child becomes a
        // zombie until *this* process reaps it, and a zombie's pid still
        // answers `kill(pid, 0)` with success -- `try_wait()` is what
        // actually reaps it and reports the exit.
        let start = std::time::Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(2),
                "stale process group {pgid} should have been killed on recovery"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn write_secret_file_is_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.token");
        write_secret_file(&path, "abc123\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "abc123\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn decisions_journal_appends_lines() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        store.append_decision_line(r#"{"ts":1}"#).unwrap();
        store.append_decision_line(r#"{"ts":2}"#).unwrap();
        let text = fs::read_to_string(store.decisions_path()).unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
