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
        let mut alive = Vec::new();
        let recovered = self.store.recover_interrupted_with(
            |task, idx| self.run_fate(task, idx, &mut alive),
            |task, idx| {
                let run_dir = self.store.run_dir(&task.id, task.attempts[idx].n);
                settle_unfinished_cost(task, idx, &run_dir, &prices);
            },
        )?;
        // Runs that outlived the daemon get a follower before any loop could
        // start a second attempt for their tasks.
        for (task_id, n) in alive {
            self.adopt_run(task_id, n);
        }
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
    /// What became of a file-backed implement run that was `running` when
    /// the daemon stopped, from the files it left. `events.jsonl` is rebuilt
    /// from the raw output either way, so the spend of a run that did not
    /// finish is still counted. A run that ended settles the attempt now;
    /// one whose process group lives is returned in `alive` to be re-adopted.
    pub(super) fn run_fate(
        &self,
        task: &mut Task,
        idx: usize,
        alive: &mut Vec<(String, u32)>,
    ) -> RunFate {
        let attempt = &task.attempts[idx];
        let events = self.store.run_dir(&task.id, attempt.n).join("events.jsonl");
        let files = RunFiles::of(&events);
        if attempt.stage != Stage::Implement || !files.raw.exists() {
            return RunFate::Interrupted;
        }
        let (_, secrets) = run_secrets(self, &task.id);
        RawTail::rebuild(&files, &events).pump(&secrets, true);
        if files.exit.exists() {
            self.settle_finished_run(task, idx, &files);
            return RunFate::Finished;
        }
        if attempt.pgid.is_some_and(group_alive) {
            alive.push((task.id.clone(), attempt.n));
            return RunFate::Alive;
        }
        RunFate::Interrupted
    }

    /// Settles an attempt whose run ended while no daemon watched: its
    /// outcome, cost and session come from the raw output and exit code the
    /// run left. The attempt is `passed` on exit 0, else `failed`; the
    /// task's own `stopped` status is set by the caller, because review and
    /// verify of this attempt belong to a task loop that no longer exists.
    fn settle_finished_run(&self, task: &mut Task, idx: usize, files: &RunFiles) {
        let attempt = &task.attempts[idx];
        let raw = std::fs::read_to_string(&files.raw).unwrap_or_default();
        let mut outcome = harness::replay_events(attempt.harness, &raw);
        let code = std::fs::read_to_string(&files.exit)
            .ok()
            .and_then(|t| t.trim().parse::<i32>().ok());
        if code != Some(0) && outcome.error.is_none() {
            let stderr = std::fs::read_to_string(&files.stderr).unwrap_or_default();
            let tail: String = stderr.trim().chars().rev().take(4000).collect();
            let tail: String = tail.chars().rev().collect();
            outcome.error = Some(if !tail.is_empty() {
                tail
            } else if let Some(code) = code {
                use std::os::unix::process::ExitStatusExt;
                harness::exit_error(
                    attempt.harness,
                    std::process::ExitStatus::from_raw(code << 8),
                )
            } else {
                "The run's exit code could not be read.".to_string()
            });
        }
        let model = (!attempt.model.is_empty()).then(|| attempt.model.clone());
        finalize_cost(self, attempt.harness, model.as_deref(), &mut outcome);
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
        let fingerprint = outcome.build_fingerprint(
            attempt.harness,
            codex_version,
            harness::prompt_hash(&argv, None),
        );
        outcome.fingerprint = Some(fingerprint.clone());
        record_run(
            self,
            &task.id,
            attempt.n,
            &CostTag::task("implement", &attempt.route_id),
            (attempt.harness, model.as_deref(), false),
            &outcome,
            attempt.started_at,
        );
        let ok = code == Some(0) && outcome.error.is_none();
        let detail = outcome.error.clone().unwrap_or_default();
        let (cost, session) = (outcome.cost_usd, outcome.session_id.clone());
        let a = &mut task.attempts[idx];
        a.ended_at = Some(now_ms());
        a.session_id = session;
        a.fingerprint = Some(fingerprint);
        a.usage = Some(Usage {
            input: outcome.usage_input,
            output: outcome.usage_output,
            cached: outcome.usage_cached,
        });
        a.cost_usd = cost;
        a.cost_estimated = outcome.cost_estimated;
        if ok {
            a.status = AttemptStatus::Passed;
        } else {
            let kind = if outcome.over_budget {
                FailureKind::Budget
            } else {
                FailureKind::Error
            };
            a.status = AttemptStatus::Failed;
            a.failure = Some(Failure {
                kind,
                signature: failure_signature(kind, &detail),
                detail,
            });
        }
        let n = a.n;
        task.cost_usd += cost.unwrap_or(0.0);
        task.decisions.push(format!(
            "Orchestrator: attempt {n} finished while the daemon was down; its result was read from the run files"
        ));
    }

    /// Takes over the run of attempt `n` of a task, which still lives: its
    /// output is followed into `events.jsonl` and the log, and when it ends
    /// the attempt is settled like one that ended while the daemon was down.
    /// The task has no loop meanwhile; `task.stop` ends the run.
    fn adopt_run(&self, task_id: String, n: u32) {
        let mut controls = self.controls.lock().unwrap();
        if controls.contains_key(&task_id) {
            return;
        }
        let cancel = CancelToken::new();
        let app = self.arc();
        let (tid, token) = (task_id.clone(), cancel.clone());
        let handle = tokio::spawn(async move { follow_adopted_run(app, tid, n, token).await });
        controls.insert(
            task_id,
            TaskControl {
                cancel,
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle,
            },
        );
    }
}

async fn follow_adopted_run(app: Arc<App>, task_id: String, n: u32, cancel: CancelToken) {
    let Ok(Some(task)) = app.store.load_task(&task_id) else {
        app.controls.lock().unwrap().remove(&task_id);
        return;
    };
    let pgid = task
        .attempts
        .iter()
        .find(|a| a.n == n)
        .and_then(|a| a.pgid)
        .unwrap_or(0);
    let harness_kind = task.attempts.iter().find(|a| a.n == n).map(|a| a.harness);
    let events = app.store.run_dir(&task_id, n).join("events.jsonl");
    let files = RunFiles::of(&events);
    let (_, secrets) = run_secrets(&app, &task_id);
    let mut tail = RawTail::rebuild(&files, &events);
    let mut notes = harness::RunOutcome::default();
    let mut caught_up = false;
    loop {
        let over = files.exit.exists() || !group_alive(pgid);
        for line in tail.pump(&secrets, over) {
            let note = harness_kind.and_then(|h| harness::feed_stream_line(h, &line, &mut notes));
            if let (true, Some(note)) = (caught_up, note) {
                app.broadcast_log(&task_id, n, note);
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
                    interrupt_adopted(&app, &task_id, n, TaskStatus::Stopped);
                }
                // A daemon shutdown leaves the run going for the next one.
                app.controls.lock().unwrap().remove(&task_id);
                return;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    let Ok(Some(mut task)) = app.store.load_task(&task_id) else {
        app.controls.lock().unwrap().remove(&task_id);
        return;
    };
    let requeue = match task.attempts.iter().position(|a| a.n == n) {
        Some(idx) if task.attempts[idx].status == AttemptStatus::Running => {
            if files.exit.exists() {
                app.settle_finished_run(&mut task, idx, &files);
                task.status = TaskStatus::Stopped;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                false
            } else {
                interrupt_adopted(&app, &task_id, n, TaskStatus::Queued);
                true
            }
        }
        _ => false,
    };
    app.controls.lock().unwrap().remove(&task_id);
    if requeue {
        app.start_task_loop(task_id.clone());
    }
    if let Ok(Some(task)) = app.store.load_task(&task_id) {
        app.advance_graph(&task.repo);
    }
}

/// An adopted run that ended without leaving its result (or was stopped):
/// the attempt is `interrupted`, its spend counted from the output.
fn interrupt_adopted(app: &App, task_id: &str, n: u32, status: TaskStatus) {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return;
    };
    let Some(idx) = task.attempts.iter().position(|a| a.n == n) else {
        return;
    };
    task.attempts[idx].status = AttemptStatus::Interrupted;
    task.attempts[idx].ended_at = Some(now_ms());
    let prices = app.settings.read().unwrap().prices.clone();
    settle_unfinished_cost(&mut task, idx, &app.store.run_dir(task_id, n), &prices);
    task.status = status;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
}
