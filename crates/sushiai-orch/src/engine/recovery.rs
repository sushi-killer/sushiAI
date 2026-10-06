use super::*;
use crate::store::RunFate;

impl App {
    /// spec step 10: attempts left `running` from a previous process
    /// become `interrupted`; their tasks become `stopped`.
    /// Every task that was in flight when the daemon stopped gets its loop
    /// back: interrupted attempts are marked and requeued by the store, and
    /// queued or drafting tasks simply resume. A `waiting` task needs no loop
    /// until its answer arrives (`task.answer` relaunches one).
    pub fn recover_on_start(&self) -> std::io::Result<()> {
        let prices = self.settings.read().unwrap().prices.clone();
        let recovered = self.store.recover_interrupted_with(
            |task, idx| self.run_fate(task, idx),
            |task, idx| {
                let run_dir = self.store.run_dir(&task.id, task.attempts[idx].n);
                settle_unfinished_cost(task, idx, &run_dir, &prices);
            },
        )?;
        for t in recovered {
            self.broadcast_task(&t);
        }
        // Spend recorded before per-run cost records existed; a no-op once done.
        crate::costs::backfill(&self.data_dir);
        for a in self.store.recover_interrupted_audits()? {
            audit::broadcast(self, &a);
        }
        for mut t in self.store.list_tasks()? {
            if t.archived {
                continue;
            }
            if matches!(
                t.status,
                TaskStatus::Queued
                    | TaskStatus::Running
                    | TaskStatus::Drafting
                    | TaskStatus::Landing
            ) {
                let id = t.id.clone();
                if settle_interrupted_advisor(&mut t, |n| self.store.run_dir(&id, n), &prices) {
                    t.updated_at = now_ms();
                    self.store.save_task(&t)?;
                    self.broadcast_task(&t);
                }
                if t.status == TaskStatus::Drafting && t.queue.backlog.is_some() {
                    // Planning resumes but never rolls into implementing.
                    self.spawn_task_loop(t.id, false);
                } else if t.status != TaskStatus::Queued || t.queue.backlog.is_none() {
                    self.start_task_loop(t.id);
                }
            }
        }
        self.spawn_landing_retries();
        self.spawn_lead_touch_check();
        // A dependency question can have become moot while the daemon was
        // down; the graph clears it now, not at the next graph event.
        let mut repos: Vec<String> = self
            .store
            .list_tasks()?
            .into_iter()
            .filter(|t| !t.archived)
            .map(|t| t.repo)
            .collect();
        repos.sort();
        repos.dedup();
        for repo in repos {
            self.advance_graph(&repo);
        }
        Ok(())
    }
}

/// Copies the lines a run's raw stdout gains into its `events.jsonl`, with
/// the project's secrets redacted, remembering how far it has read.
struct RawTail {
    raw: PathBuf,
    events: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl RawTail {
    /// Starts at the top: `events.jsonl` is rebuilt from the raw file, so it
    /// is complete whatever the previous daemon had written before it died.
    fn rebuild(files: &RunFiles, events: &Path) -> Self {
        let _ = std::fs::write(events, b"");
        RawTail {
            raw: files.raw.clone(),
            events: events.to_path_buf(),
            offset: 0,
            partial: Vec::new(),
        }
    }

    /// The redacted lines written since the last call; `last` also flushes a
    /// final line that has no newline.
    fn pump(&mut self, secrets: &HashMap<String, String>, last: bool) -> Vec<String> {
        use std::io::{Read, Seek};
        let Ok(mut file) = std::fs::File::open(&self.raw) else {
            return Vec::new();
        };
        let mut fresh = Vec::new();
        if file.seek(std::io::SeekFrom::Start(self.offset)).is_ok() {
            let _ = file.read_to_end(&mut fresh);
        }
        self.offset += fresh.len() as u64;
        self.partial.extend(fresh);
        let mut lines = Vec::new();
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&line[..end]).into_owned());
        }
        if last && !self.partial.is_empty() {
            lines.push(String::from_utf8_lossy(&std::mem::take(&mut self.partial)).into_owned());
        }
        lines
            .into_iter()
            .map(|l| {
                let l = redact(&l, secrets);
                append_line(&self.events, &l);
                l
            })
            .collect()
    }
}

/// Stops a run's process group: SIGTERM, SIGKILL after 5 s.
async fn stop_group(pgid: i32) {
    unsafe {
        libc::killpg(pgid, libc::SIGTERM);
    }
    for _ in 0..50 {
        if !group_alive(pgid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

impl App {
    /// What becomes of a file-backed implement run that was `running` when
    /// the daemon stopped, from the files it left. `events.jsonl` is rebuilt
    /// from the raw output either way, so the spend of a run that did not
    /// finish is still counted. A run that ended (its exit file exists) or
    /// whose process group lives is resumed: the task's loop follows it and
    /// carries on with review and verify like after any other pass.
    pub(super) fn run_fate(&self, task: &Task, idx: usize) -> RunFate {
        let attempt = &task.attempts[idx];
        let events = self.store.run_dir(&task.id, attempt.n).join("events.jsonl");
        let files = RunFiles::of(&events);
        if attempt.stage != Stage::Implement || !files.raw.exists() {
            return RunFate::Interrupted;
        }
        let (_, secrets) = run_secrets(self, &task.id);
        RawTail::rebuild(&files, &events).pump(&secrets, true);
        if files.exit.exists() || attempt.pgid.is_some_and(group_alive) {
            return RunFate::Resume;
        }
        RunFate::Interrupted
    }
}

/// The attempt of `task` whose run a restarted daemon has to follow: the
/// last implement attempt, still `running`, with its run files in place.
pub(super) fn resumable_attempt(app: &App, task: &Task) -> Option<usize> {
    let idx = task.attempts.len().checked_sub(1)?;
    let attempt = &task.attempts[idx];
    let events = app.store.run_dir(&task.id, attempt.n).join("events.jsonl");
    (attempt.stage == Stage::Implement
        && attempt.status == AttemptStatus::Running
        && RunFiles::of(&events).raw.exists())
    .then_some(idx)
}

/// Follows the run of attempt `idx`, which started under an earlier daemon:
/// its output goes into `events.jsonl` and the log until it ends, then its
/// outcome is read from what it left, as `run_harness` would have returned it.
/// A daemon shutdown leaves the run going; `task.stop` ends it.
pub(super) async fn follow_run(
    app: &Arc<App>,
    task: &Task,
    idx: usize,
    cancel: &CancelToken,
) -> Result<harness::RunOutcome, RunError> {
    let attempt = &task.attempts[idx];
    let (n, pgid) = (attempt.n, attempt.pgid.unwrap_or(0));
    let events = app.store.run_dir(&task.id, n).join("events.jsonl");
    let files = RunFiles::of(&events);
    let (_, secrets) = run_secrets(app, &task.id);
    let mut tail = RawTail::rebuild(&files, &events);
    let mut notes = harness::RunOutcome::default();
    let mut caught_up = false;
    loop {
        let over = files.exit.exists() || !group_alive(pgid);
        for line in tail.pump(&secrets, over) {
            let note = harness::feed_stream_line(attempt.harness, &line, &mut notes);
            if let (true, Some(note)) = (caught_up, note) {
                app.broadcast_log(&task.id, n, note);
            }
        }
        caught_up = true;
        if over {
            break;
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                if !app.shutting_down.load(Ordering::SeqCst) {
                    stop_group(pgid).await;
                    tail.pump(&secrets, true);
                }
                return Err(RunError::Cancelled);
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    let code = std::fs::read_to_string(&files.exit)
        .ok()
        .and_then(|t| t.trim().parse::<i32>().ok());
    if code.is_none() {
        return Err(RunError::Io(
            "The run ended without leaving an exit code.".into(),
        ));
    }
    let raw = std::fs::read_to_string(&files.raw).unwrap_or_default();
    let mut outcome = harness::replay_events(attempt.harness, &raw);
    if code != Some(0) && outcome.error.is_none() {
        let stderr = std::fs::read_to_string(&files.stderr).unwrap_or_default();
        let tail: String = stderr.trim().chars().rev().take(4000).collect();
        let tail: String = tail.chars().rev().collect();
        outcome.error = Some(if !tail.is_empty() {
            tail
        } else {
            use std::os::unix::process::ExitStatusExt;
            harness::exit_error(
                attempt.harness,
                std::process::ExitStatus::from_raw(code.unwrap_or(1) << 8),
            )
        });
    }
    let model = (!attempt.model.is_empty()).then(|| attempt.model.clone());
    finalize_cost(app, attempt.harness, model.as_deref(), &mut outcome);
    let meta: serde_json::Value = std::fs::read_to_string(&files.meta)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let argv: Vec<String> = meta
        .get("argv")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let codex_version = meta
        .get("codexVersion")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    outcome.fingerprint = Some(outcome.build_fingerprint(
        attempt.harness,
        codex_version,
        harness::prompt_hash(&argv, None),
    ));
    // A run the earlier daemon already settled (it died during review) has its record.
    if attempt.fingerprint.is_none() {
        record_run(
            app,
            &task.id,
            n,
            &CostTag::task("implement", &attempt.route_id),
            (attempt.harness, model.as_deref(), false),
            &outcome,
            attempt.started_at,
        );
    }
    Ok(outcome)
}
