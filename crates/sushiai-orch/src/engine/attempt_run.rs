use super::*;

/// What a run is for, so its cost record names the stage. `repo` is set for
/// a run that is no task's (audit, evolution proposal, chat): its `task_id`
/// argument is then the audit or proposal id, kept as the record's `runId`.
pub(super) struct CostTag {
    pub stage: &'static str,
    pub route_id: String,
    pub repo: Option<String>,
}

impl CostTag {
    pub fn task(stage: &'static str, route_id: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: None,
        }
    }

    pub fn repo(stage: &'static str, route_id: &str, repo: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: Some(repo.to_string()),
        }
    }
}

/// The files a file-backed run (an implement or plan attempt) leaves next to
/// its `events.jsonl`: the child's stdin, its raw stdout and stderr, the exit
/// code its wrapper writes last, the argv it was started with, and the token
/// its Stop hook presents (mode 0600). They let a restarted daemon find the
/// run again and finish it (see `recovery.rs`).
pub(super) struct RunFiles {
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

/// `true` while some process of group `pgid` is alive.
pub(super) fn group_alive(pgid: i32) -> bool {
    pgid > 0 && unsafe { libc::kill(pgid, 0) == 0 }
}

/// `true` while group `pgid` is alive and is the run whose wrapper shell was
/// given `exit` as its `$0`. A pgid saved by an earlier daemon can have been
/// reused by a stranger's process since.
///
/// `ps` can fail to name a live process under load, so an unnamed group that
/// still exists is asked twice more before it counts as a stranger's.
pub(super) fn run_group_alive(pgid: i32, exit: &Path) -> bool {
    for pause_ms in [0, 50, 100] {
        std::thread::sleep(Duration::from_millis(pause_ms));
        if !group_alive(pgid) {
            return false;
        }
        if store::group_started_with(pgid, exit) {
            return true;
        }
    }
    false
}

/// Creates `path` empty, readable by the owner only.
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Removes the unredacted output files (`*.raw`, `*.stderr.log`) of a run
/// directory. Only a settled attempt that nothing can resume loses them.
pub(super) fn remove_raw_output(run_dir: &Path) {
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

/// How a run relates to its attempt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RunTrack {
    /// A nested run (review, triage, audit, chat): pipes, no recovery.
    No,
    /// The attempt's own run, on files: its pgid and session are saved. A
    /// daemon shutdown kills it.
    Attempt,
    /// As `Attempt`, and a daemon shutdown leaves it running for the next
    /// daemon to adopt (the implement run).
    Resumable,
}

/// What ends a run early: silence, a loop, or spend past the cap. The same
/// guard watches a run this daemon started and one it adopted.
pub(super) struct Guard {
    stall: Option<Stall>,
    detector: Option<LoopDetector>,
    /// Dollars one attempt may spend; `None` = no cap.
    cap: Option<f64>,
    last_output: tokio::time::Instant,
}

/// Why a guard stopped a run.
pub(super) enum Trip {
    Stalled(String),
    Looped(String),
    OverBudget(String),
}

impl Trip {
    pub fn apply(self, outcome: &mut harness::RunOutcome) {
        match self {
            Trip::Stalled(error) => {
                outcome.stalled = true;
                outcome.error = Some(error);
            }
            Trip::Looped(note) => outcome.looped = Some(note),
            Trip::OverBudget(error) => {
                outcome.over_budget = true;
                outcome.error = Some(error);
            }
        }
    }
}

impl Guard {
    /// No watch: a nested run.
    pub fn off() -> Self {
        Guard {
            stall: None,
            detector: None,
            cap: None,
            last_output: tokio::time::Instant::now(),
        }
    }

    /// The watch a variant asks for; `hook_running` pauses the stall clock.
    pub fn for_variant(variant: &Variant, hook_running: Option<Arc<AtomicBool>>) -> Self {
        Guard {
            stall: (variant.stall_timeout_secs > 0).then(|| Stall {
                limit: Duration::from_secs(variant.stall_timeout_secs),
                paused: hook_running.unwrap_or_default(),
            }),
            detector: variant.loop_detect.then(LoopDetector::new),
            cap: (variant.max_attempt_cost_usd > 0.0).then_some(variant.max_attempt_cost_usd),
            last_output: tokio::time::Instant::now(),
        }
    }

    pub(super) fn armed(&self) -> bool {
        self.stall.is_some()
    }

    /// When the silence limit runs out. An unset limit still needs a
    /// deadline that does not overflow `Instant` (select! builds a disabled
    /// branch's future too).
    pub(super) fn deadline(&self) -> tokio::time::Instant {
        let limit = self
            .stall
            .as_ref()
            .map_or(Duration::from_secs(365 * 24 * 3600), |s| s.limit);
        self.last_output + limit
    }

    pub(super) fn heard(&mut self) {
        self.last_output = tokio::time::Instant::now();
    }

    /// The silence limit ran out. Silence is the only stall signal; a long
    /// Bash call or the Stop hook's verify (up to 540s) is silent too, so the
    /// limit must sit above them and the clock pauses while the hook runs.
    pub(super) fn stall_expired(&mut self) -> Option<Trip> {
        let stall = self.stall.as_ref()?;
        if stall.paused.load(Ordering::SeqCst) {
            self.heard();
            return None;
        }
        Some(Trip::Stalled(format!(
            "No output for {}s; the run was stopped as stalled.",
            stall.limit.as_secs()
        )))
    }

    /// One more output line (`outcome` already has it): `Some` when the run
    /// must stop.
    pub(super) fn line(
        &mut self,
        app: &App,
        (harness, model): (Harness, Option<&str>),
        line: &str,
        outcome: &harness::RunOutcome,
    ) -> Option<Trip> {
        self.heard();
        if let Some(hit) = self.detector.as_mut().and_then(|d| d.feed(line)) {
            return Some(Trip::Looped(format!(
                "{} Do not repeat it; change approach.",
                hit.detail
            )));
        }
        let cap = self.cap?;
        let spent = streamed_spend(app, harness, model, outcome);
        (outcome.over_budget || spent.is_some_and(|c| c > cap)).then(|| {
            Trip::OverBudget(format!(
                "The attempt spent about ${:.4}, past its ${cap} cap; the run was stopped.",
                spent.unwrap_or(cap)
            ))
        })
    }
}

/// Reads the lines `file` gains, as they arrive, until the run is over: its
/// exit file exists or its process group is gone. What was written before
/// that moment is still delivered.
fn tail_file(
    file: PathBuf,
    exit: PathBuf,
    pgid: i32,
) -> Box<dyn tokio::io::AsyncRead + Unpin + Send> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut tx, rx) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut offset = 0u64;
        loop {
            let over = exit.exists() || !group_alive(pgid);
            if let Ok(mut f) = tokio::fs::File::open(&file).await {
                let _ =
                    tokio::io::AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(offset)).await;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match f.read(&mut buf).await {
                        Ok(n) if n > 0 => {
                            offset += n as u64;
                            if tx.write_all(&buf[..n]).await.is_err() {
                                return; // the reader gave up (cancelled run)
                            }
                        }
                        _ => break,
                    }
                }
            }
            if over {
                let _ = tx.shutdown().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    Box::new(rx)
}

pub(super) enum RunError {
    Cancelled,
    Io(String),
    /// The harness binary does not exist: nothing ran, so it costs no attempt.
    NotFound(String),
}

/// Spawn `claude`/`codex` in its own process group, write `brief` to
/// stdin, stream stdout into `events.jsonl` and the log broadcast, and wait
/// for it to exit. `cancel` triggers SIGTERM to the group, then SIGKILL
/// after 5s (spec: "Each child in its own process group; stop = SIGTERM to
/// the group, SIGKILL after 5s").
///
/// `track` gates the mid-run `pgid`/`session_id` persistence below:
/// `Attempt`/`Resumable` for a session that *is* `task.attempts[_]` with
/// number `attempt_n` (the implement or plan attempt itself); `No` for a nested session
/// that merely borrows that attempt's number for its run directory (review,
/// orchestrator triage) -- otherwise its own pgid/session id would
/// overwrite the real attempt's, leaving it stuck `running` with a session
/// id `--resume` should never see and a pgid recovery would `killpg` on a
/// process that already exited (P1, triage review). A nested session's own
/// child is still reachable through `cancel` for as long as this call is
/// awaited, so skipping persistence here only affects recovery after a
/// daemon crash mid-review/-triage, not the normal stop path.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_harness(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    tag: CostTag,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    mut guard: Guard,
) -> Result<harness::RunOutcome, RunError> {
    let track_attempt = track != RunTrack::No;
    let started_at = now_ms();
    let mut argv = harness::build_argv(req);
    let mut bin = resolve_binary(req.harness);
    #[cfg(unix)]
    if req.harness == Harness::Claude {
        if let Some(settings_path) = req.settings_path {
            let token_path = settings_path.with_file_name("oauth-token");
            if token_path.is_file() {
                let claude = bin;
                let args = argv;
                bin = "/bin/sh".to_string();
                argv = vec![
                    "-c".into(),
                    "exec 3<\"$1\"; rm -f \"$1\"; export CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3; shift; exec \"$@\"".into(),
                    "sushiai".into(),
                    token_path.to_string_lossy().into_owned(),
                    claude,
                ];
                argv.extend(args);
            }
        }
    }
    // Asked before the run, not after, so a stand-in binary that logs its
    // argv per invocation ends with the run's own.
    let codex_version = match req.harness {
        Harness::Codex => codex_version(&bin).await,
        Harness::Claude => None,
    };
    let mut cmd;
    let (project_env, redaction_env) = run_secrets(app, task_id);
    let files = RunFiles::of(events_path);
    // What the fingerprint hashes: the harness's own argv, not the wrapper's.
    let run_argv = argv.clone();
    if track_attempt {
        // stdin, stdout and stderr are files and a wrapper writes the exit
        // code last, so the run does not depend on this process staying
        // alive (a restarted daemon re-adopts it by pgid).
        let wrapped = std::mem::take(&mut argv);
        argv = vec![
            "-c".into(),
            "\"$@\"; c=$?; echo $c > \"$0.tmp\" && mv \"$0.tmp\" \"$0\"; exit $c".into(),
            files.exit.to_string_lossy().into_owned(),
            bin,
        ];
        argv.extend(wrapped);
        bin = "/bin/sh".to_string();
        {
            use std::io::Write;
            let _ =
                create_private(&files.stdin).and_then(|mut f| f.write_all(brief_text.as_bytes()));
        }
        let _ = std::fs::remove_file(&files.exit);
        let _ = std::fs::write(
            &files.meta,
            json!({"argv": &run_argv, "codexVersion": codex_version}).to_string(),
        );
    }
    cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .current_dir(worktree)
        .env("PATH", augmented_path())
        // Lets a repo's own hooks tell an orchd-run agent from a person's
        // session (sushiAI's lesson reminder stays quiet for it).
        .env("ORCHD_TASK", task_id)
        .env("SUSHIAI_HOME", &app.home)
        .envs(&project_env);
    if let Some(token) = app.hook_token_of(task_id, attempt_n) {
        cmd.env("SUSHIAI_ORCH_TOKEN", token);
    }
    if track_attempt {
        match (
            std::fs::File::open(&files.stdin).map(std::process::Stdio::from),
            create_private(&files.raw).map(std::process::Stdio::from),
            create_private(&files.stderr).map(std::process::Stdio::from),
        ) {
            (Ok(i), Ok(o), Ok(e)) => cmd.stdin(i).stdout(o).stderr(e),
            _ => return Err(RunError::Io("could not open the run files".into())),
        };
    } else {
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(|e| {
        let text = format!("{bin}: {e}");
        if e.kind() == std::io::ErrorKind::NotFound {
            RunError::NotFound(text)
        } else {
            RunError::Io(text)
        }
    })?;
    let pgid = child.id().map(|p| p as i32);
    if track_attempt {
        if let Some(pgid) = pgid {
            persist_attempt_field(app, task_id, attempt_n, move |a| a.pgid = Some(pgid)).await;
        }
    }

    type Source = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
    let (stdout, stderr): (Source, Source) = if track_attempt {
        let pgid = pgid.unwrap_or(0);
        (
            tail_file(files.raw.clone(), files.exit.clone(), pgid),
            tail_file(files.stderr.clone(), files.exit.clone(), pgid),
        )
    } else {
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(brief_text.as_bytes()).await;
            drop(stdin);
        }
        (
            Box::new(child.stdout.take().expect("piped stdout")),
            Box::new(child.stderr.take().expect("piped stderr")),
        )
    };
    let mut out_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));

    let mut outcome = harness::RunOutcome::default();
    let harness_kind = req.harness;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stderr_tail = String::new();
    let mut session_persisted = false;
    let model = req.model;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                if track == RunTrack::Resumable && app.runs_survive_shutdown() {
                    // A daemon shutdown leaves the run going: the next daemon
                    // finds it by its process group and files.
                    return Err(RunError::Cancelled);
                }
                kill_group(pgid, &mut child).await;
                // A stopped run still ran: keep what it reported so far.
                let fp = fingerprint_of(&outcome, req, &run_argv, codex_version.clone());
                let mut partial = outcome.clone();
                finalize_cost(app, req.harness, req.model, &mut partial);
                partial.fingerprint = Some(fp.clone());
                record_run(
                    app,
                    task_id,
                    attempt_n,
                    &tag,
                    (req.harness, req.model, false),
                    &partial,
                    started_at,
                );
                if track_attempt {
                    persist_attempt_field(app, task_id, attempt_n, move |a| a.fingerprint = Some(fp)).await;
                } else if let Ok(json) = serde_json::to_string(&fp) {
                    // Nested runs (review, advisor, audit) have no attempt to
                    // write to and RunError::Cancelled carries no payload:
                    // hand the fingerprint to the caller through a sidecar.
                    let _ = std::fs::write(cancelled_fingerprint_path(events_path), json);
                }
                return Err(RunError::Cancelled);
            }
            _ = tokio::time::sleep_until(guard.deadline()), if guard.armed() => {
                let Some(trip) = guard.stall_expired() else { continue };
                kill_group(pgid, &mut child).await;
                trip.apply(&mut outcome);
                break;
            }
            line = out_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(l)) => {
                        let l = redact(&l, &redaction_env);
                        append_line(events_path, &l);
                        if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                            app.broadcast_log(task_id, attempt_n, note);
                        }
                        if let Some(trip) = guard.line(app, (harness_kind, model), &l, &outcome) {
                            kill_group(pgid, &mut child).await;
                            trip.apply(&mut outcome);
                            break;
                        }
                        if track_attempt && !session_persisted {
                            if let Some(sid) = outcome.session_id.clone() {
                                session_persisted = true;
                                persist_attempt_field(app, task_id, attempt_n, move |a| a.session_id = Some(sid)).await;
                            }
                        }
                    }
                    _ => { stdout_done = true; }
                }
            }
            line = err_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(l)) => {
                        guard.heard();
                        let l = redact(&l, &redaction_env);
                        append_line(events_path, &format!("[stderr] {l}"));
                        stderr_tail.push_str(&l);
                        stderr_tail.push('\n');
                        if stderr_tail.len() > 4000 {
                            let cut = stderr_tail.len() - 4000;
                            let cut = (cut..stderr_tail.len()).find(|i| stderr_tail.is_char_boundary(*i)).unwrap_or(0);
                            stderr_tail.drain(..cut);
                        }
                    }
                    _ => { stderr_done = true; }
                }
            }
            status = child.wait(), if stdout_done && stderr_done => {
                if let Ok(status) = status {
                    // The wrapper shell reports a harness it could not start
                    // (missing binary or interpreter) as 126 or 127: nothing ran.
                    if track_attempt && matches!(status.code(), Some(126 | 127)) {
                        let tail = stderr_tail.trim();
                        return Err(RunError::NotFound(if tail.is_empty() {
                            format!("{:?}: command not found", req.harness)
                        } else {
                            tail.to_string()
                        }));
                    }
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            crate::harness::exit_error(req.harness, status)
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    finalize_cost(app, req.harness, req.model, &mut outcome);
    outcome.fingerprint = Some(fingerprint_of(&outcome, req, &run_argv, codex_version));
    record_run(
        app,
        task_id,
        attempt_n,
        &tag,
        (req.harness, req.model, false),
        &outcome,
        started_at,
    );
    Ok(outcome)
}

/// The project's agent variables, and every value known for the project
/// (agent and MCP variables) that a run's output must not keep.
pub(super) fn run_secrets(
    app: &App,
    task_id: &str,
) -> (HashMap<String, String>, HashMap<String, String>) {
    app.store
        .load_task(task_id)
        .ok()
        .flatten()
        .and_then(|task| {
            task.project_id.or_else(|| {
                app.secrets
                    .read()
                    .unwrap()
                    .repo_projects
                    .get(&task.repo)
                    .cloned()
            })
        })
        .map_or_else(Default::default, |id| {
            let secrets = app.secrets.read().unwrap();
            let agent = secrets.projects.get(&id).cloned().unwrap_or_default();
            let mut known = agent.clone();
            known.extend(secrets.project_mcp.get(&id).cloned().unwrap_or_default());
            (agent, known)
        })
}

pub(super) fn redact(line: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut values: Vec<_> = env.values().filter(|value| !value.is_empty()).collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.into_iter().fold(line.to_string(), |text, value| {
        text.replace(value, "[REDACTED]")
    })
}

/// Prices a run the harness reported no cost for: Codex from its token
/// usage, Claude from its streamed messages (marked estimated).
pub(super) fn finalize_cost(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &mut harness::RunOutcome,
) {
    if outcome.cost_usd.is_none() && harness == Harness::Codex {
        let price = model.and_then(|m| app.settings.read().unwrap().prices.get(m).copied());
        outcome.cost_usd = price.map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        });
    }
    if harness == Harness::Claude {
        outcome.estimate_cost(&app.settings.read().unwrap().prices);
    }
}

/// Appends the run's cost record. A resumed Claude session's own total
/// covers its earlier runs too, so its record is priced from this run's
/// streamed messages when it has any.
pub(super) fn record_run(
    app: &App,
    task_id: &str,
    attempt_n: u32,
    tag: &CostTag,
    run: (Harness, Option<&str>, bool),
    outcome: &harness::RunOutcome,
    started_at: i64,
) {
    let (harness, req_model, resumed) = run;
    let (mut cost, mut estimated) = (outcome.cost_usd.unwrap_or(0.0), outcome.cost_estimated);
    if harness == Harness::Claude && resumed {
        if let Some(own) = outcome.streamed_cost(&app.settings.read().unwrap().prices) {
            cost = own;
            estimated = true;
        }
    }
    let is_task = tag.repo.is_none();
    let repo = tag.repo.clone().unwrap_or_else(|| {
        app.store
            .load_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.repo)
            .unwrap_or_default()
    });
    let model = outcome
        .fingerprint
        .as_ref()
        .and_then(|f| f.models.first().cloned())
        .or_else(|| req_model.map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    let ended = now_ms();
    crate::costs::append(
        &app.data_dir,
        &crate::costs::CostRecord {
            ts: ended,
            repo,
            task_id: is_task.then(|| task_id.to_string()),
            run_id: (!is_task && !task_id.is_empty()).then(|| task_id.to_string()),
            stage: tag.stage.to_string(),
            attempt: is_task.then_some(attempt_n),
            route_id: tag.route_id.clone(),
            harness,
            model,
            cost_usd: cost,
            estimated,
            input_tokens: outcome.usage_input,
            cached_tokens: outcome.usage_cached,
            output_tokens: outcome.usage_output,
            ms: (ended - started_at).max(0) as u64,
            backfilled: false,
        },
    );
}

/// What this run has cost so far, priced from its streamed message usage.
/// The CLI's own `total_cost_usd` is not used: on a resumed session it also
/// covers earlier attempts, which the cap must not count.
fn streamed_spend(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &harness::RunOutcome,
) -> Option<f64> {
    let settings = app.settings.read().unwrap();
    match harness {
        Harness::Claude => outcome.streamed_cost(&settings.prices),
        Harness::Codex => model.and_then(|m| settings.prices.get(m)).map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        }),
    }
}

fn fingerprint_of(
    outcome: &harness::RunOutcome,
    req: &harness::RunRequest<'_>,
    argv: &[String],
    codex_version: Option<String>,
) -> Fingerprint {
    let settings_json = req
        .settings_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    outcome.build_fingerprint(
        req.harness,
        codex_version,
        harness::prompt_hash(argv, settings_json.as_ref()),
    )
}

/// `codex --version`, asked once per binary per daemon start.
async fn codex_version(bin: &str) -> Option<String> {
    static CACHE: OnceLock<StdMutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap().get(bin) {
        return known.clone();
    }
    let version = tokio::process::Command::new(bin)
        .arg("--version")
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty());
    cache
        .lock()
        .unwrap()
        .insert(bin.to_string(), version.clone());
    version
}

fn cancelled_fingerprint_path(events_path: &Path) -> PathBuf {
    events_path.with_extension("fingerprint.json")
}

/// The fingerprint a cancelled nested run left next to its events file.
pub(super) fn read_cancelled_fingerprint(events_path: &Path) -> Option<Fingerprint> {
    let path = cancelled_fingerprint_path(events_path);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::redact;
    use std::collections::HashMap;

    #[test]
    fn redacts_known_values_longest_first() {
        let env = HashMap::from([
            ("TOKEN".to_string(), "invented-secret-value".to_string()),
            ("SHORT".to_string(), "xyz".to_string()),
        ]);
        assert_eq!(
            redact("invented-secret-value xyz", &env),
            "[REDACTED] [REDACTED]"
        );
    }
}
